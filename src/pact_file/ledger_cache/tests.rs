use std::sync::Arc;

use super::{LedgerCacheKey, get, put};
use crate::store::ContextTreeLedger;

fn empty_ledger(dir: &std::path::Path) -> Arc<ContextTreeLedger> {
    let path = dir.join("ledger-source.pact");
    let mut tree = crate::ContextTree::with_id("ledger_cache").expect("tree");
    tree.save_file(&path).expect("save");
    Arc::new(crate::pact_file::open_ledger(&path).expect("open ledger"))
}

fn key(path: &std::path::Path, generation: u64, committed_len: u64) -> LedgerCacheKey {
    LedgerCacheKey::new(
        path,
        &std::fs::metadata(path).expect("metadata"),
        "identity".to_string(),
        generation,
        committed_len,
    )
}

#[test]
fn entry_is_weak_and_dies_with_its_last_owner() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("a.pact");
    std::fs::write(&path, b"x").expect("write");
    let ledger = empty_ledger(temp.path());
    let k = key(&path, 3, 4096);
    put(k.clone(), &ledger);
    let hit = get(&k).expect("live entry");
    assert!(Arc::ptr_eq(&hit, &ledger));
    assert_eq!(
        Arc::strong_count(&ledger),
        2,
        "the cache adds no strong count"
    );
    drop(hit);
    drop(ledger);
    assert!(get(&k).is_none(), "the last owner released the ledger");
}

#[test]
fn same_size_replacement_generation_or_identity_change_never_hits() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("b.pact");
    std::fs::write(&path, b"same size").expect("write");
    let ledger = empty_ledger(temp.path());
    let original = key(&path, 7, 999);
    put(original.clone(), &ledger);
    assert!(get(&original).is_some());

    assert!(get(&key(&path, 8, 999)).is_none());
    assert!(get(&key(&path, 7, 1000)).is_none());

    let mut other_identity = original.clone();
    other_identity.file_identity = "other".to_string();
    assert!(get(&other_identity).is_none());

    let replacement = temp.path().join("b.replacement");
    std::fs::write(&replacement, b"same size").expect("write replacement");
    std::fs::rename(&replacement, &path).expect("atomic replace");
    assert!(get(&key(&path, 7, 999)).is_none());
    drop(ledger);
}
