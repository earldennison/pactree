use std::io::Cursor;

use super::{
    ByteSpanSource, ScratchBackend, ScratchFile, ScratchPages, ScratchRange, SpanTable, SpoolSpan,
};
use crate::pact_file::btree::{CowBTree, PAGE_BYTES, PageReader};

#[test]
#[cfg(not(unix))]
fn portable_reads_use_an_independent_serialized_cursor() {
    use std::io::Write;
    use std::sync::Mutex;

    let mut file = tempfile::NamedTempFile::new().expect("temp file");
    let bytes: Vec<u8> = (0..128).collect();
    file.write_all(&bytes).expect("write");
    let reader = Mutex::new(file.reopen().expect("independent read handle"));
    std::thread::scope(|scope| {
        for offset in [0, 16, 48, 96] {
            let reader = &reader;
            let bytes = &bytes;
            scope.spawn(move || {
                for _ in 0..100 {
                    let mut actual = [0; 16];
                    super::read_exact_at_portable(reader, &mut actual, offset).expect("read");
                    assert_eq!(actual, bytes[offset as usize..offset as usize + 16]);
                }
            });
        }
    });
    let error = super::read_exact_at_portable(&reader, &mut [0; 4], 126)
        .expect_err("short file cannot satisfy exact read");
    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    super::read_exact_at_portable(&reader, &mut [], 128).expect("empty read");
    file.write_all(b"tailtail").expect("append after readers");
    assert_eq!(
        std::fs::read(file.path()).expect("physical bytes"),
        [bytes.as_slice(), b"tailtail"].concat()
    );
}

#[test]
fn flushed_reads_do_not_move_the_append_cursor() {
    let mut spool = ScratchFile::create(None, 4).expect("scratch");
    spool.append(b"abcdefghijklmnop").expect("initial append");
    assert_eq!(
        spool
            .read_span(SpoolSpan { offset: 1, len: 4 })
            .expect("read"),
        b"bcde"
    );
    spool.append(b"qrst").expect("append after read");
    spool.flush().expect("flush");
    assert_eq!(
        std::fs::read(spool.path().expect("path")).expect("physical bytes"),
        b"abcdefghijklmnopqrst"
    );
}

#[test]
fn concurrent_flushed_reads_keep_their_requested_offsets() {
    let mut spool = ScratchFile::create(None, 8).expect("scratch");
    let bytes: Vec<u8> = (0..128).collect();
    spool.append(&bytes).expect("append");
    std::thread::scope(|scope| {
        for offset in [0, 16, 48, 96] {
            let spool = &spool;
            let bytes = &bytes;
            scope.spawn(move || {
                for _ in 0..100 {
                    assert_eq!(
                        spool
                            .read_span(SpoolSpan { offset, len: 16 })
                            .expect("read"),
                        bytes[offset as usize..offset as usize + 16]
                    );
                }
            });
        }
    });
    spool.append(b"tailtail").expect("append after readers");
    assert_eq!(
        std::fs::read(spool.path().expect("path")).expect("physical bytes"),
        [bytes.as_slice(), b"tailtail"].concat()
    );
}

#[test]
fn spool_appends_reads_compares_and_copies_in_chunks() {
    let mut spool = ScratchFile::create(None, 7).expect("scratch");
    let path = spool.path().expect("temp path").to_path_buf();
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "scratch is user-only");
    }
    let first = spool.append(b"hello, ").expect("append");
    let second = spool.append(b"world").expect("append");
    assert_eq!(first, SpoolSpan { offset: 0, len: 7 });
    assert_eq!(second, SpoolSpan { offset: 7, len: 5 });
    assert_eq!(spool.read_span(second).expect("read"), b"world");
    assert!(spool.span_eq(first, b"hello, ").expect("eq"));
    assert!(!spool.span_eq(first, b"hello!!").expect("neq"));
    assert!(!spool.span_eq(first, b"hello").expect("length differs"));
    let mut out = Vec::new();
    spool
        .copy_span(SpoolSpan { offset: 0, len: 12 }, &mut out)
        .expect("copy");
    assert_eq!(out, b"hello, world");
    assert!(
        spool.read_span(SpoolSpan { offset: 8, len: 8 }).is_err(),
        "a span past the end is rejected"
    );
    drop(spool);
    assert!(!path.exists(), "dropping the scratch removes its file");
}

#[test]
fn streamed_append_is_chunked_and_limit_checked() {
    let mut spool = ScratchFile::create(None, 1024).expect("scratch");
    let big = vec![b'z'; 3 * 1024 * 1024 + 17];
    let span = spool
        .append_from_reader(&mut Cursor::new(&big), 4 * 1024 * 1024)
        .expect("stream in");
    assert_eq!(span.len, big.len() as u64);
    assert!(spool.span_eq(span, &big).expect("eq"));
    let over = spool.append_from_reader(&mut Cursor::new(&big), 1024);
    assert!(over.is_err(), "the limit is enforced while streaming");
}

struct FullDisk {
    capacity: usize,
    written: Vec<u8>,
}

impl ScratchBackend for FullDisk {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.written.len() + bytes.len() > self.capacity {
            return Err(std::io::Error::other("no space left on device"));
        }
        self.written.extend_from_slice(bytes);
        Ok(())
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
        let start = offset as usize;
        buf.copy_from_slice(&self.written[start..start + buf.len()]);
        Ok(())
    }
}

#[test]
fn disk_full_is_an_error_that_leaves_the_spool_length_unchanged() {
    let mut spool = ScratchFile::with_backend(
        Box::new(FullDisk {
            capacity: 10,
            written: Vec::new(),
        }),
        4,
    );
    spool.append(b"12345678").expect("fits");
    let error = spool
        .append(b"wxyz")
        .expect_err("a chunk-sized append reaches the disk");
    assert!(matches!(error, crate::PactError::Io(_)));
    assert_eq!(spool.len(), 8, "a failed append is not accounted");
    assert!(
        spool
            .span_eq(SpoolSpan { offset: 0, len: 8 }, b"12345678")
            .expect("intact")
    );

    spool.append(b"xy").expect("buffered");
    spool.append(b"z").expect("buffered");
    assert_eq!(spool.len(), 11);
    assert_eq!(
        spool
            .read_span(SpoolSpan { offset: 6, len: 5 })
            .expect("straddling read"),
        b"78xyz"
    );
    let error = spool
        .append(b"wq")
        .expect_err("flushing the buffer hits the full disk");
    assert!(matches!(error, crate::PactError::Io(_)));
    assert_eq!(spool.len(), 11, "the failing append is not accounted");
    assert_eq!(
        spool
            .read_span(SpoolSpan { offset: 8, len: 3 })
            .expect("buffered bytes stay readable"),
        b"xyz"
    );
}

#[test]
fn buffered_spool_reads_back_every_fragment_in_any_order() {
    let mut spool = ScratchFile::create(None, 64).expect("spool");
    let mut mirror = Vec::new();
    let mut spans = Vec::new();
    let mut seed = 7_u64;
    for round in 0..2_000_u64 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let len = (seed >> 33) as usize % 150;
        let bytes = (0..len)
            .map(|i| (round + i as u64) as u8)
            .collect::<Vec<_>>();
        spans.push(spool.append(&bytes).expect("append"));
        mirror.extend_from_slice(&bytes);
        if round % 97 == 0 {
            let probe = spans[(round as usize) / 3];
            assert_eq!(
                spool.read_span(probe).expect("read"),
                mirror[probe.offset as usize..(probe.offset + probe.len) as usize]
            );
        }
    }
    for (index, span) in spans.iter().enumerate().rev() {
        let expected = &mirror[span.offset as usize..(span.offset + span.len) as usize];
        assert_eq!(
            spool.read_span(*span).expect("read"),
            expected,
            "span {index}"
        );
        assert!(spool.span_eq(*span, expected).expect("eq"));
    }
    let mut out = Vec::new();
    spool
        .copy_span(
            SpoolSpan {
                offset: 0,
                len: spool.len(),
            },
            &mut out,
        )
        .expect("copy");
    assert_eq!(out, mirror);
}

#[test]
fn scratch_pages_back_a_cow_btree_with_a_bounded_cache() {
    let mut pages = ScratchPages::create(None, 4096, 4 * PAGE_BYTES + 4 * 200).expect("pages");
    let entries = (0..300_u32)
        .map(|n| (n.to_be_bytes().to_vec(), vec![n as u8; 40]))
        .collect::<Vec<_>>();
    let tree = CowBTree::bulk_load(&mut pages, entries.clone()).expect("bulk load");
    assert!(pages.pages_written() > 4);
    assert!(
        pages.cache_resident_bytes() <= 4 * PAGE_BYTES + 4 * 200,
        "cache stays within its budget"
    );
    for (key, value) in &entries {
        assert_eq!(
            tree.get(&pages, key).expect("get").as_deref(),
            Some(value.as_slice())
        );
    }
    let root = tree.root_address().expect("root");
    assert!(
        root >= PAGE_BYTES as u64,
        "the reserved first page keeps addresses non-zero"
    );
    assert_eq!(
        pages.read_page_cached(root).expect("cached read"),
        pages.read_page(root).expect("read")
    );
}

#[test]
fn span_table_dedupes_by_exact_bytes_even_under_forced_hash_collisions() {
    let mut pages = ScratchPages::create(None, 4096, 1 << 16).expect("pages");
    let mut table = SpanTable::with_hasher(|_| [7_u8; 16]);
    assert_eq!(table.insert(&mut pages, b"alpha", 1).expect("insert"), 1);
    assert_eq!(table.insert(&mut pages, b"beta", 2).expect("insert"), 2);
    assert_eq!(table.insert(&mut pages, b"alphb", 3).expect("insert"), 3);
    assert_eq!(
        table.insert(&mut pages, b"alpha", 99).expect("duplicate"),
        1
    );
    assert_eq!(table.lookup(&pages, b"beta").expect("lookup"), Some(2));
    assert_eq!(table.lookup(&pages, b"alphb").expect("lookup"), Some(3));
    assert_eq!(table.lookup(&pages, b"alph").expect("lookup"), None);
    assert_eq!(table.lookup(&pages, b"gamma").expect("lookup"), None);

    let mut real = SpanTable::new();
    let huge = vec![b'q'; 2 * 1024 * 1024];
    assert_eq!(real.insert(&mut pages, &huge, 10).expect("insert huge"), 10);
    assert_eq!(real.lookup(&pages, &huge).expect("lookup huge"), Some(10));
    let mut almost = huge.clone();
    almost[huge.len() - 1] = b'r';
    assert_eq!(
        real.lookup(&pages, &almost).expect("lookup near miss"),
        None
    );
}

#[test]
fn keyset_range_pages_cover_every_entry_once_and_stop() {
    let mut pages = ScratchPages::create(None, 4096, 1 << 16).expect("pages");
    let entries = (0..1000_u32)
        .map(|n| (n.to_be_bytes().to_vec(), vec![1_u8]))
        .collect::<Vec<_>>();
    let tree = CowBTree::bulk_load(&mut pages, entries).expect("bulk load");
    let mut range = ScratchRange::new(&tree, &[], &[0xff; 4], 64);
    let mut seen = Vec::new();
    let mut page_count = 0;
    loop {
        let page = range.next_page(&pages).expect("page");
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 64);
        page_count += 1;
        seen.extend(page.into_iter().map(|(key, _)| key));
    }
    assert_eq!(seen.len(), 1000);
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(page_count, 16);
    assert!(range.next_page(&pages).expect("exhausted").is_empty());
}
