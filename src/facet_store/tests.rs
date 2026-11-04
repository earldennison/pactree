use std::cell::Cell;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use super::{
    FACET_COPY_BUFFER_BYTES, FACET_HEADER_BYTES, FACET_ROOT_SLOT_BYTES, FACET_ROOT_SLOT_COUNT,
    FACET_ROOT_SLOT_DIGEST_START, FACET_ROOT_SLOT_START, FacetCompactionPolicy, FacetFaultPoint,
    FacetReader, FacetSegmentType, FacetStore, FacetWriteBatch, decode_root_slot, facet_path_for,
};
use crate::error::PactErrorKind;

thread_local! {
    static FACET_COMPACTION_PEAK_CHUNK_BYTES: Cell<usize> = const { Cell::new(0) };
}

fn reset_facet_compaction_peak_chunk_bytes() {
    FACET_COMPACTION_PEAK_CHUNK_BYTES.with(|peak| peak.set(0));
}

fn facet_compaction_peak_chunk_bytes() -> usize {
    FACET_COMPACTION_PEAK_CHUNK_BYTES.with(Cell::get)
}

pub(super) fn record_facet_compaction_chunk_bytes(bytes: usize) {
    FACET_COMPACTION_PEAK_CHUNK_BYTES.with(|peak| peak.set(peak.get().max(bytes)));
}

fn make_source(dir: &Path) -> PathBuf {
    let source = dir.join("data.pact");
    std::fs::write(&source, b"stand-in source pact file").expect("write source");
    source
}

fn code_batch(entries: &[(u16, &[u8])]) -> FacetWriteBatch {
    let mut batch = FacetWriteBatch::new();
    for (code, bytes) in entries {
        batch.append(FacetSegmentType::Code(*code), bytes.to_vec());
    }
    batch
}

fn read_header_bytes(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).expect("read facet file");
    bytes[..FACET_HEADER_BYTES].to_vec()
}

#[test]
fn facet_path_always_ends_in_pact_sem() {
    assert_eq!(
        facet_path_for(Path::new("/tmp/data.pact")),
        PathBuf::from("/tmp/data.pact.sem")
    );
    assert_eq!(
        facet_path_for(Path::new("/tmp/data")),
        PathBuf::from("/tmp/data.pact.sem")
    );
}

#[test]
fn create_then_open_roundtrips_an_empty_store() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    assert_eq!(store.path(), facet_path_for(&source).as_path());
    assert_eq!(store.source_path(), source.as_path());

    let reopened =
        FacetStore::open(&source, FacetCompactionPolicy::disabled()).expect("open facet");
    let stats = reopened.stats().expect("stats");
    assert_eq!(stats.generation, 1);
    assert_eq!(stats.live_bytes, 0);
    assert_eq!(stats.dead_bytes, 0);
    assert!(stats.by_type.is_empty());
    assert!(reopened.reader().expect("reader").segments().is_empty());
}

#[test]
fn create_refuses_missing_source_and_existing_facet() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let missing = temp.path().join("missing.pact");
    assert!(FacetStore::create(&missing, FacetCompactionPolicy::disabled()).is_err());

    let source = make_source(temp.path());
    FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("first create");
    assert!(FacetStore::create(&source, FacetCompactionPolicy::disabled()).is_err());
}

#[test]
fn open_rejects_foreign_magic() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut bogus = vec![0_u8; FACET_HEADER_BYTES + 16];
    bogus[..8].copy_from_slice(b"PACTBIN2");
    std::fs::write(facet_path_for(&source), &bogus).expect("write bogus facet");
    let error = FacetStore::open(&source, FacetCompactionPolicy::disabled())
        .expect_err("foreign magic must be rejected");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn append_then_read_back_typed_segments() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");

    let mut batch = FacetWriteBatch::new();
    batch.append(FacetSegmentType::Code(7), b"occurrence-table".to_vec());
    batch.append(
        FacetSegmentType::Name("lexical".to_string()),
        b"postings".to_vec(),
    );
    let stats = store.commit(&batch).expect("commit");
    assert_eq!(stats.generation, 2);
    assert_eq!(
        stats.live_bytes,
        (b"occurrence-table".len() + b"postings".len()) as u64
    );

    let reader = store.reader().expect("reader");
    let segments = reader.segments();
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].id, 1);
    assert_eq!(segments[0].segment_type, FacetSegmentType::Code(7));
    assert_eq!(segments[1].id, 2);
    assert_eq!(
        segments[1].segment_type,
        FacetSegmentType::Name("lexical".to_string())
    );
    assert_eq!(
        reader.read_segment(1).expect("segment 1"),
        b"occurrence-table"
    );
    assert_eq!(reader.read_segment(2).expect("segment 2"), b"postings");
    assert_eq!(
        reader.read_segment(99).expect_err("unknown id").kind(),
        PactErrorKind::NotFound
    );
}

#[test]
fn individual_segment_checksums_detect_corruption() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    store
        .commit(&code_batch(&[(1, b"alpha-payload"), (2, b"beta-payload")]))
        .expect("commit");

    let facet = facet_path_for(&source);
    let mut bytes = std::fs::read(&facet).expect("read facet");
    let position = bytes
        .windows(b"alpha-payload".len())
        .position(|window| window == b"alpha-payload")
        .expect("locate first payload");
    bytes[position] ^= 0xFF;
    std::fs::write(&facet, &bytes).expect("corrupt payload");

    let reader = store.reader().expect("reader");
    let error = reader.read_segment(1).expect_err("corrupt segment");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);

    assert_eq!(reader.read_segment(2).expect("segment 2"), b"beta-payload");
}

#[test]
fn commits_alternate_between_the_two_root_slots() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    let facet = facet_path_for(&source);

    let header = read_header_bytes(&facet);
    assert_eq!(decode_root_slot(&header, 0).expect("slot 0").generation, 1);
    assert!(decode_root_slot(&header, 1).is_none());

    store.commit(&code_batch(&[(1, b"one")])).expect("commit 2");
    let header = read_header_bytes(&facet);
    assert_eq!(decode_root_slot(&header, 0).expect("slot 0").generation, 1);
    assert_eq!(decode_root_slot(&header, 1).expect("slot 1").generation, 2);

    store.commit(&code_batch(&[(1, b"two")])).expect("commit 3");
    let header = read_header_bytes(&facet);
    assert_eq!(decode_root_slot(&header, 0).expect("slot 0").generation, 3);
    assert_eq!(decode_root_slot(&header, 1).expect("slot 1").generation, 2);
}

#[test]
fn torn_newest_root_falls_back_to_prior_slot() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    store
        .commit(&code_batch(&[(1, b"kept")]))
        .expect("commit 2");
    store
        .commit(&code_batch(&[(2, b"newest")]))
        .expect("commit 3");

    let facet = facet_path_for(&source);
    let mut bytes = std::fs::read(&facet).expect("read facet");
    bytes[FACET_ROOT_SLOT_START + FACET_ROOT_SLOT_DIGEST_START] ^= 0xFF;
    std::fs::write(&facet, &bytes).expect("tear newest root");

    let reader = store.reader().expect("reader falls back");
    assert_eq!(reader.generation(), 2);
    let segments = reader.segments();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].id, 1);
    assert_eq!(reader.read_segment(1).expect("prior segment"), b"kept");
}

#[test]
fn commit_recovers_from_a_valid_root_with_an_unreadable_manifest() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    store
        .commit(&code_batch(&[(1, b"kept")]))
        .expect("commit 2");
    store
        .commit(&code_batch(&[(2, b"unreadable")]))
        .expect("commit 3");

    let facet = facet_path_for(&source);
    let header = read_header_bytes(&facet);
    let newest = decode_root_slot(&header, 0).expect("generation 3 root");
    assert_eq!(newest.generation, 3);
    let mut bytes = std::fs::read(&facet).expect("read facet");
    bytes[usize::try_from(newest.manifest_address).expect("manifest address")] ^= 0xFF;
    std::fs::write(&facet, bytes).expect("corrupt newest manifest only");

    assert_eq!(store.reader().expect("fallback reader").generation(), 2);
    store
        .commit(&code_batch(&[(3, b"recovered")]))
        .expect("commit from newest readable root");
    let reader = store.reader().expect("reader after recovery");
    assert_eq!(reader.generation(), 3);
    assert_eq!(reader.read_segment(1).expect("kept"), b"kept");
    assert_eq!(reader.read_segment(2).expect("recovered"), b"recovered");
}

#[test]
fn successful_commit_truncates_an_abandoned_physical_tail() {
    use std::io::Write;

    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    store
        .commit(&code_batch(&[(1, b"published")]))
        .expect("seed commit");

    let facet = facet_path_for(&source);
    let mut file = OpenOptions::new()
        .append(true)
        .open(&facet)
        .expect("open abandoned tail");
    file.write_all(&vec![0xA5; 1024 * 1024])
        .expect("append abandoned bytes");
    file.sync_all().expect("sync abandoned bytes");
    assert!(
        std::fs::metadata(&facet).expect("bloated metadata").len()
            > store
                .stats()
                .expect("published stats")
                .file_length_watermark
    );

    let stats = store
        .commit(&FacetWriteBatch::new())
        .expect("reclaim abandoned tail");
    assert_eq!(
        std::fs::metadata(&facet).expect("reclaimed metadata").len(),
        stats.file_length_watermark
    );
    assert_eq!(
        store.reader().expect("reader").read_segment(1).unwrap(),
        b"published"
    );
}

#[test]
fn commit_waits_for_the_current_facet_writer() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    let guard = OpenOptions::new()
        .read(true)
        .write(true)
        .open(facet_path_for(&source))
        .expect("open writer guard");
    guard.lock().expect("lock facet");

    let (tx, rx) = mpsc::channel();
    let mut concurrent = store.clone();
    let worker = std::thread::spawn(move || {
        let result = concurrent.commit(&code_batch(&[(1, b"serialized")]));
        tx.send(result).expect("send commit result");
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "facet commit must not pass another writer's lock"
    );

    guard.unlock().expect("unlock facet");
    rx.recv_timeout(Duration::from_secs(5))
        .expect("commit resumes after unlock")
        .expect("serialized commit");
    worker.join().expect("commit worker");
    assert_eq!(
        store.reader().expect("reader").read_segment(1).unwrap(),
        b"serialized"
    );
}

#[test]
fn concurrent_commits_preserve_every_batch() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    let barrier = Arc::new(Barrier::new(8));
    let mut workers = Vec::new();
    for value in 0_u8..8 {
        let barrier = Arc::clone(&barrier);
        let mut concurrent = store.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            concurrent
                .commit(&code_batch(&[(u16::from(value), &[value])]))
                .expect("serialized concurrent commit");
        }));
    }
    for worker in workers {
        worker.join().expect("commit worker");
    }

    let reader = store.reader().expect("reader");
    assert_eq!(reader.segments().len(), 8);
    let mut payloads = reader
        .segments()
        .iter()
        .map(|segment| reader.read_segment(segment.id).expect("segment")[0])
        .collect::<Vec<_>>();
    payloads.sort_unstable();
    assert_eq!(payloads, (0_u8..8).collect::<Vec<_>>());
}

#[test]
fn replaced_facet_inode_is_detected_as_a_stale_writer_target() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    store
        .commit(&code_batch(&[(1, b"old inode")]))
        .expect("seed old inode");
    let stale = OpenOptions::new()
        .read(true)
        .write(true)
        .open(facet_path_for(&source))
        .expect("open old inode");

    let mut aggressive =
        FacetStore::open(&source, FacetCompactionPolicy::new(0.0)).expect("open aggressive");
    aggressive
        .commit(&FacetWriteBatch::new())
        .expect("replace through compaction");

    assert!(
        !super::path_still_points_to_file(&facet_path_for(&source), &stale).expect("compare inode")
    );
}

#[test]
fn locked_facet_identity_check_preserves_the_writer_cursor() {
    use std::io::{Seek, SeekFrom};

    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.path())
        .expect("writer");
    file.lock().expect("exclusive lock");
    file.seek(SeekFrom::Start(17)).expect("position writer");
    assert!(super::path_still_points_to_file(store.path(), &file).expect("same locked file"));
    assert_eq!(file.stream_position().expect("writer cursor"), 17);
}

#[test]
fn byte_identical_replacement_is_not_the_original_writer_target() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    let original = OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.path())
        .expect("original writer target");
    let bytes = std::fs::read(store.path()).expect("original bytes");
    crate::atomic_output::publish_bytes(
        store.path(),
        &bytes,
        crate::atomic_output::PublishMode::ReplaceExisting,
    )
    .expect("replace with identical bytes");
    assert!(
        !super::path_still_points_to_file(store.path(), &original).expect("compare file identity")
    );
}

#[test]
fn compaction_reader_uses_the_existing_locked_writer_handle() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    store
        .commit(&code_batch(&[(1, b"locked payload")]))
        .expect("seed");
    let file = super::open_locked_current(store.path()).expect("locked writer");
    let mut reader = FacetReader::from_file(file).expect("read through lock owner");
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.path())
        .expect("competing writer");
    assert!(matches!(
        contender.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    let mut actual = Vec::new();
    super::stream_verified_segment(&mut reader.file, &reader.manifest.entries[0], |chunk| {
        actual.extend_from_slice(chunk);
        Ok(())
    })
    .expect("stream through lock owner");
    assert_eq!(actual, b"locked payload");
}

#[test]
fn open_fails_when_both_root_slots_are_torn() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    store.commit(&code_batch(&[(1, b"one")])).expect("commit");

    let facet = facet_path_for(&source);
    let mut bytes = std::fs::read(&facet).expect("read facet");
    for slot in 0..FACET_ROOT_SLOT_COUNT {
        bytes[FACET_ROOT_SLOT_START
            + slot * FACET_ROOT_SLOT_BYTES
            + FACET_ROOT_SLOT_DIGEST_START] ^= 0xFF;
    }
    std::fs::write(&facet, &bytes).expect("tear both roots");

    let error = store.reader().expect_err("no valid root");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn old_reader_survives_atomic_replacement() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    store
        .commit(&code_batch(&[(1, b"old-payload")]))
        .expect("commit");

    let pinned = store.reader().expect("pinned reader");
    let pinned_generation = pinned.generation();

    let mut aggressive =
        FacetStore::open(&source, FacetCompactionPolicy::new(0.0)).expect("open aggressive");
    let mut batch = FacetWriteBatch::new();
    batch.drop_segment(1);
    batch.append(FacetSegmentType::Code(2), b"new-payload".to_vec());
    let stats = aggressive.commit(&batch).expect("compacting commit");
    assert_eq!(stats.dead_bytes, 0, "compaction must remove dead bytes");

    assert_eq!(pinned.generation(), pinned_generation);
    assert_eq!(pinned.read_segment(1).expect("old segment"), b"old-payload");

    let fresh = store.reader().expect("fresh reader");
    assert!(fresh.generation() > pinned_generation);
    assert_eq!(fresh.segments().len(), 1);
    assert_eq!(fresh.segments()[0].id, 2);
    assert_eq!(fresh.read_segment(2).expect("new segment"), b"new-payload");
    assert_eq!(
        fresh.read_segment(1).expect_err("dropped segment").kind(),
        PactErrorKind::NotFound
    );
}

#[cfg(unix)]
#[test]
fn facet_permissions_match_or_tighten_the_source() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640))
        .expect("chmod source");

    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::new(0.0)).expect("create facet");
    let facet = facet_path_for(&source);
    let mode = std::fs::metadata(&facet)
        .expect("facet metadata")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode & !0o640, 0, "facet mode {mode:o} exceeds source 0640");

    let mut batch = FacetWriteBatch::new();
    batch.append(FacetSegmentType::Code(1), b"payload".to_vec());
    store.commit(&batch).expect("commit");
    let mut drop_batch = FacetWriteBatch::new();
    drop_batch.drop_segment(1);
    store.commit(&drop_batch).expect("compacting commit");
    let mode = std::fs::metadata(&facet)
        .expect("facet metadata")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode & !0o640, 0, "facet mode {mode:o} exceeds source 0640");

    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o400))
        .expect("tighten source");
    store
        .commit(&FacetWriteBatch::new())
        .expect("clamping commit");
    let mode = std::fs::metadata(&facet)
        .expect("facet metadata")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode & !0o400, 0, "facet mode {mode:o} exceeds source 0400");
}

#[test]
fn live_dead_accounting_tracks_types_and_drops() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");

    let mut batch = FacetWriteBatch::new();
    batch.append(FacetSegmentType::Code(1), vec![0xAA; 100]);
    batch.append(FacetSegmentType::Code(1), vec![0xBB; 50]);
    batch.append(
        FacetSegmentType::Name("coverage".to_string()),
        vec![0xCC; 25],
    );
    let stats = store.commit(&batch).expect("commit");
    assert_eq!(stats.live_bytes, 175);
    assert_eq!(
        stats.by_type,
        vec![
            super::FacetTypeStats {
                segment_type: FacetSegmentType::Code(1),
                segment_count: 2,
                live_bytes: 150,
            },
            super::FacetTypeStats {
                segment_type: FacetSegmentType::Name("coverage".to_string()),
                segment_count: 1,
                live_bytes: 25,
            },
        ]
    );

    let mut drop_batch = FacetWriteBatch::new();
    drop_batch.drop_segment(2);
    let stats = store.commit(&drop_batch).expect("drop commit");
    assert_eq!(stats.live_bytes, 125);
    assert!(
        stats.dead_bytes >= 50,
        "dropped payload must be accounted dead, got {}",
        stats.dead_bytes
    );
    assert_eq!(stats.by_type.len(), 2);
    assert_eq!(stats.by_type[0].segment_count, 1);
    assert_eq!(stats.by_type[0].live_bytes, 100);
}

#[test]
fn dropping_an_unknown_segment_is_not_found() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    let mut batch = FacetWriteBatch::new();
    batch.drop_segment(42);
    let error = store.commit(&batch).expect_err("unknown drop");
    assert_eq!(error.kind(), PactErrorKind::NotFound);
}

#[test]
fn amplification_past_policy_triggers_automatic_compaction() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::new(0.5)).expect("create facet");

    store
        .commit(&code_batch(&[(1, &[0xAA; 4000]), (2, &[0xBB; 100])]))
        .expect("seed commit");

    let mut batch = FacetWriteBatch::new();
    batch.drop_segment(1);
    let stats = store.commit(&batch).expect("compacting commit");
    assert_eq!(stats.dead_bytes, 0);
    assert_eq!(stats.live_bytes, 100);

    let facet_len = std::fs::metadata(facet_path_for(&source))
        .expect("metadata")
        .len();
    assert_eq!(facet_len, stats.file_length_watermark);

    let reader = store.reader().expect("reader");
    assert_eq!(reader.read_segment(2).expect("survivor"), vec![0xBB; 100]);
}

#[test]
fn compaction_copies_large_segments_in_bounded_windows() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let payload = vec![0x5A; FACET_COPY_BUFFER_BYTES * 3 + 17];
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
    let mut seed = FacetWriteBatch::new();
    seed.append(FacetSegmentType::Code(1), payload.clone());
    store.commit(&seed).expect("seed large segment");

    reset_facet_compaction_peak_chunk_bytes();
    let mut aggressive =
        FacetStore::open(&source, FacetCompactionPolicy::new(0.0)).expect("open aggressive");
    aggressive
        .commit(&FacetWriteBatch::new())
        .expect("streaming compaction");
    let peak = facet_compaction_peak_chunk_bytes();
    assert!(peak > 0, "compaction must exercise the streaming copy loop");
    assert!(
        peak <= FACET_COPY_BUFFER_BYTES,
        "compaction copied {peak} bytes in one window"
    );
    assert_eq!(
        aggressive
            .reader()
            .expect("reader")
            .read_segment(1)
            .unwrap(),
        payload
    );
}

#[test]
fn compaction_below_policy_threshold_does_not_run() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::new(1000.0)).expect("create facet");
    store
        .commit(&code_batch(&[(1, &[0xAA; 64])]))
        .expect("seed");
    let mut batch = FacetWriteBatch::new();
    batch.drop_segment(1);
    batch.append(FacetSegmentType::Code(2), vec![0xBB; 64]);
    let stats = store.commit(&batch).expect("commit");
    assert!(stats.dead_bytes > 0, "lenient policy must keep dead bytes");
}

#[test]
fn every_write_publish_rename_fault_reopens_to_a_published_root() {
    for fault in [
        FacetFaultPoint::SegmentsWritten,
        FacetFaultPoint::DataSynced,
        FacetFaultPoint::ManifestWritten,
        FacetFaultPoint::ManifestSynced,
        FacetFaultPoint::RootSlotWritten,
        FacetFaultPoint::RootSynced,
        FacetFaultPoint::CompactionStaged,
        FacetFaultPoint::CompactionPublished,
    ] {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = make_source(temp.path());
        let mut store =
            FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");
        store
            .commit(&code_batch(&[(1, b"alpha")]))
            .expect("seed commit");

        let mut aggressive =
            FacetStore::open(&source, FacetCompactionPolicy::new(0.0)).expect("open aggressive");
        let mut batch = FacetWriteBatch::new();
        batch.drop_segment(1);
        batch.append(FacetSegmentType::Code(2), b"beta".to_vec());
        let error = aggressive
            .commit_with_fault(&batch, Some(fault))
            .expect_err("injected fault");
        assert_eq!(error.kind(), PactErrorKind::Io, "fault {fault:?}");

        let reader = FacetReader::open(&facet_path_for(&source))
            .unwrap_or_else(|error| panic!("reopen after {fault:?} failed: {error}"));
        let generation = reader.generation();
        assert!(
            (2..=4).contains(&generation),
            "unexpected generation {generation} after {fault:?}"
        );
        if generation == 2 {
            assert_eq!(reader.segments().len(), 1);
            assert_eq!(reader.read_segment(1).expect("prior segment"), b"alpha");
        } else {
            assert_eq!(reader.segments().len(), 1);
            assert_eq!(reader.read_segment(2).expect("new segment"), b"beta");
            assert_eq!(
                reader.read_segment(1).expect_err("dropped").kind(),
                PactErrorKind::NotFound
            );
        }
        for info in reader.segments() {
            reader
                .read_segment(info.id)
                .unwrap_or_else(|error| panic!("segment {} after {fault:?}: {error}", info.id));
        }
    }
}

#[test]
fn empty_and_invalid_inputs_are_rejected_deterministically() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let source = make_source(temp.path());
    let mut store =
        FacetStore::create(&source, FacetCompactionPolicy::disabled()).expect("create facet");

    let mut batch = FacetWriteBatch::new();
    batch.append(FacetSegmentType::Name(String::new()), b"x".to_vec());
    assert_eq!(
        store.commit(&batch).expect_err("empty name").kind(),
        PactErrorKind::Invalid
    );

    assert_eq!(
        FacetStore::open(&source, FacetCompactionPolicy::new(f64::NAN))
            .expect_err("NaN policy")
            .kind(),
        PactErrorKind::Invalid
    );
    assert_eq!(
        FacetStore::open(&source, FacetCompactionPolicy::new(-1.0))
            .expect_err("negative policy")
            .kind(),
        PactErrorKind::Invalid
    );

    let stats = store.commit(&FacetWriteBatch::new()).expect("empty commit");
    assert_eq!(stats.generation, 2);
    assert!(FacetWriteBatch::new().is_empty());
}
