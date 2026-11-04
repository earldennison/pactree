use super::{WriterLock, tail_is_unpublished_candidate};

#[test]
fn lock_paths_are_siblings_and_exclusive() {
    let temp = tempfile::tempdir().expect("tempdir");
    let canonical = temp.path().join("file.pact");
    std::fs::write(&canonical, b"x").expect("write");
    let lock_path = WriterLock::lock_path(&canonical);
    assert_eq!(lock_path, temp.path().join("file.pact.lock"));
    let first = WriterLock::acquire(&canonical).expect("first lock");
    assert_eq!(first.path(), lock_path);
    let second = WriterLock::acquire(&canonical);
    assert!(second.is_err(), "the lock is exclusive while held");
    drop(first);
    WriterLock::acquire(&canonical).expect("released");
}

#[test]
fn only_a_next_generation_candidate_counts_as_a_recoverable_tail() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("tail.pact");
    let mut bytes = vec![0_u8; 100];

    let mut header = Vec::new();
    header.extend_from_slice(crate::pact_file::codec::RECORD_MAGIC);
    header.push(crate::pact_file::codec::RecordKind::CompactTransaction as u8);
    header.push(crate::pact_file::codec::RECORD_VERSION);
    header.extend_from_slice(&0_u16.to_le_bytes());
    header.extend_from_slice(&8_u64.to_le_bytes());
    header.extend_from_slice(&0_u64.to_le_bytes());
    header.extend_from_slice(&[0; 8]);
    bytes.extend_from_slice(&header);
    std::fs::write(&path, &bytes).expect("write");
    let mut file = std::fs::File::open(&path).expect("open");
    assert!(tail_is_unpublished_candidate(&mut file, 100, 7).expect("probe"));
    assert!(!tail_is_unpublished_candidate(&mut file, 100, 9).expect("probe"));
    assert!(tail_is_unpublished_candidate(&mut file, 100 + header.len() as u64, 7).unwrap());

    std::fs::write(&path, [vec![0_u8; 100], vec![0xaa; 64]].concat()).expect("write");
    let mut file = std::fs::File::open(&path).expect("open");
    assert!(!tail_is_unpublished_candidate(&mut file, 100, 7).expect("probe"));

    std::fs::write(&path, [vec![0_u8; 100], vec![0xaa; 5]].concat()).expect("write");
    let mut file = std::fs::File::open(&path).expect("open");
    assert!(tail_is_unpublished_candidate(&mut file, 100, 7).expect("probe"));
}
