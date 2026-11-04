use super::*;

#[test]
fn persisted_candidates_are_not_marked_temporary() {
    use crate::atomic_output::{PublishMode, publish_staged, stage_bytes};
    use std::os::windows::fs::MetadataExt;

    let temp = tempfile::tempdir().expect("tempdir");
    for mode in [PublishMode::CreateNew, PublishMode::ReplaceExisting] {
        let destination = temp.path().join(format!("{mode:?}.pact"));
        if mode == PublishMode::ReplaceExisting {
            std::fs::write(&destination, b"old").expect("seed");
        }
        let staged = stage_bytes(&destination, b"durable").expect("stage");
        assert_ne!(
            staged
                .as_file()
                .metadata()
                .expect("staged metadata")
                .file_attributes()
                & 0x100,
            0
        );
        publish_staged(staged, &destination, mode).expect("publish");
        assert_eq!(
            std::fs::metadata(&destination)
                .expect("published metadata")
                .file_attributes()
                & 0x100,
            0
        );
        assert_eq!(
            std::fs::read(destination).expect("published bytes"),
            b"durable"
        );
    }
}

#[test]
fn failed_staged_publication_cleans_candidate_and_preserves_destination() {
    use crate::atomic_output::{PublishMode, publish_staged, stage_bytes};
    let temp = tempfile::tempdir().expect("tempdir");
    let destination = temp.path().join("destination");
    std::fs::create_dir(&destination).expect("directory");
    std::fs::write(destination.join("sentinel"), b"untouched").expect("sentinel");
    let staged = stage_bytes(&destination, b"candidate").expect("stage");
    let candidate = staged.path().to_path_buf();
    assert!(publish_staged(staged, &destination, PublishMode::ReplaceExisting).is_err());
    assert!(
        !candidate.exists(),
        "rejected staged candidate must be cleaned"
    );
    assert_eq!(
        std::fs::read(destination.join("sentinel")).expect("sentinel"),
        b"untouched"
    );
}

#[test]
fn rename_info_matches_the_windows_abi() {
    let pointer = std::mem::size_of::<usize>();
    assert_eq!(std::mem::offset_of!(RenameInfo, flags), 0);
    assert_eq!(std::mem::offset_of!(RenameInfo, root), pointer);
    assert_eq!(std::mem::offset_of!(RenameInfo, name_bytes), 2 * pointer);
    assert_eq!(std::mem::offset_of!(RenameInfo, name), 2 * pointer + 4);
    assert_eq!(
        std::mem::align_of::<RenameInfo>(),
        std::mem::align_of::<usize>()
    );
}

#[test]
fn invalid_name_leaves_source_and_destination_untouched() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    std::fs::write(&source, b"candidate").expect("source");
    std::fs::write(&destination, b"old").expect("destination");
    let invalid = temp.path().join("destination\0ignored");
    assert_eq!(
        replace_file(&source, &invalid)
            .expect_err("invalid name")
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        std::fs::read(source).expect("source survives"),
        b"candidate"
    );
    assert_eq!(
        std::fs::read(destination).expect("destination survives"),
        b"old"
    );
}

#[test]
fn unicode_name_is_length_delimited_and_preserves_existing_reader() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("candidate");
    let destination = temp.path().join("snapshot-λ-🦀.pact");
    std::fs::write(&source, b"candidate").expect("source");
    std::fs::write(&destination, b"old").expect("destination");
    let old = std::fs::File::open(&destination).expect("old handle");
    let old_id = crate::file_identity::file_id(&old).expect("old identity");
    replace_file(&source, &destination).expect("unicode rename");
    assert_eq!(
        std::fs::read(&destination).expect("new bytes"),
        b"candidate"
    );
    assert_ne!(
        crate::file_identity::path_id(&destination).expect("new identity"),
        old_id
    );
    assert_eq!(
        crate::file_identity::file_id(&old).expect("pinned identity"),
        old_id
    );
    assert!(!source.exists());
}
