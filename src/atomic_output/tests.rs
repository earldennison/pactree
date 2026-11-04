use std::error::Error;
use std::fs;
use std::io::ErrorKind;

use super::*;

#[test]
fn atomic_replace_preserves_live_mapped_readers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let destination = dir.path().join("mapped.pact");
    fs::write(&destination, b"old snapshot").expect("seed");
    let old = File::open(&destination).expect("old reader");
    let mapping = unsafe { memmap2::MmapOptions::new().map(&old) }.expect("map old file");

    publish_bytes(&destination, b"new snapshot", PublishMode::ReplaceExisting)
        .expect("replace mapped file");

    assert_eq!(&mapping[..], b"old snapshot");
    assert_eq!(fs::read(&destination).expect("new reader"), b"new snapshot");
}

#[test]
fn tail_recovery_preserves_mapped_prefix_and_reopens_append_owner() {
    use std::io::{Seek, SeekFrom};
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tail.pact");
    fs::write(&path, b"committed").expect("seed");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("writer");
    let mapping = unsafe { memmap2::MmapOptions::new().len(9).map(&file) }.expect("map prefix");
    file.seek(SeekFrom::End(0)).expect("seek tail");
    file.write_all(b"abandoned candidate").expect("write tail");
    truncate_for_append(&path, &mut file, 9, 3).expect("recover tail");
    assert_eq!(&mapping[..], b"committed");
    assert_eq!(file.metadata().expect("writer metadata").len(), 9);
    assert_eq!(
        crate::file_identity::file_id(&file).expect("writer identity"),
        crate::file_identity::path_id(&path).expect("path identity")
    );
    file.seek(SeekFrom::End(0)).expect("seek append");
    file.write_all(b"new").expect("append");
    assert_eq!(fs::read(&path).expect("read"), b"committednew");
    assert_eq!(&mapping[..], b"committed");
}

#[test]
fn unchanged_append_length_never_replaces_the_physical_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("unchanged.pact");
    fs::write(&path, b"committed").expect("seed");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("writer");
    let before = crate::file_identity::file_id(&file).expect("identity");
    let mapping = unsafe { memmap2::MmapOptions::new().map(&file) }.expect("map");
    assert_eq!(
        truncate_for_append(&path, &mut file, 9, 3).expect("no truncation"),
        0
    );
    assert_eq!(
        before,
        crate::file_identity::path_id(&path).expect("same identity")
    );
    assert_eq!(&mapping[..], b"committed");
}

#[test]
fn failed_replacement_does_not_remove_destination() {
    let dir = tempfile::tempdir().expect("tempdir");
    let destination = dir.path().join("preserved.pact");
    fs::write(&destination, b"committed").expect("seed");
    assert!(replace_file(&dir.path().join("missing"), &destination).is_err());
    assert_eq!(
        fs::read(destination).expect("destination remains"),
        b"committed"
    );
}

#[test]
fn rejected_native_replacement_preserves_both_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("candidate");
    let destination = dir.path().join("destination");
    fs::write(&source, b"candidate bytes").expect("candidate");
    fs::create_dir(&destination).expect("destination directory");
    fs::write(destination.join("sentinel"), b"untouched").expect("sentinel");
    assert!(replace_file(&source, &destination).is_err());
    assert_eq!(
        fs::read(source).expect("candidate remains"),
        b"candidate bytes"
    );
    assert_eq!(
        fs::read(destination.join("sentinel")).expect("destination remains"),
        b"untouched"
    );
}

#[test]
fn atomic_create_never_replaces_existing_destination() {
    let dir = tempfile::tempdir().expect("tempdir");
    let destination = dir.path().join("artifact.pact");
    fs::write(&destination, b"old").expect("seed destination");

    let error = publish_bytes(&destination, b"new", PublishMode::CreateNew)
        .expect_err("exclusive create must reject an existing destination");

    assert_eq!(error.kind(), crate::PactErrorKind::Io);
    assert_eq!(
        error
            .source()
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .map(std::io::Error::kind),
        Some(ErrorKind::AlreadyExists)
    );
    assert_eq!(fs::read(&destination).expect("read destination"), b"old");
}

#[test]
fn atomic_replace_publishes_complete_new_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let destination = dir.path().join("artifact.pact");
    fs::write(&destination, b"old").expect("seed destination");
    let replacement = vec![0x5a; 128 * 1024];

    publish_bytes(&destination, &replacement, PublishMode::ReplaceExisting)
        .expect("replace destination");

    assert_eq!(
        fs::read(&destination).expect("read destination"),
        replacement
    );
}

#[test]
fn atomic_replace_preserves_old_file_before_publish() {
    let dir = tempfile::tempdir().expect("tempdir");
    let destination = dir.path().join("artifact.pact");
    fs::write(&destination, b"old").expect("seed destination");

    let staged = stage_bytes(&destination, b"new").expect("stage replacement");
    assert_eq!(fs::read(&destination).expect("read destination"), b"old");

    drop(staged);
    assert_eq!(fs::read(&destination).expect("read destination"), b"old");
}
