use std::error::Error;
use std::fs;
use std::io::ErrorKind;

use super::*;

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
