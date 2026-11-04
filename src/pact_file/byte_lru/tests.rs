use super::{ByteLru, ENTRY_OVERHEAD_BYTES};

fn charge(value_bytes: usize) -> usize {
    value_bytes + std::mem::size_of::<u64>() + ENTRY_OVERHEAD_BYTES
}

#[test]
fn zero_budget_caches_nothing_and_returns_the_value() {
    let mut cache = ByteLru::<u64, Vec<u8>>::new(0);
    assert!(!cache.fits(1));
    assert_eq!(cache.insert(1, vec![b'a'], 1), Err(vec![b'a']));
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.resident_bytes(), 0);
    assert!(cache.get(&1).is_none());
}

#[test]
fn touching_protects_an_entry_and_eviction_releases_its_charge() {
    let mut cache = ByteLru::<u64, Vec<u8>>::new(3 * charge(4));
    cache.insert(1, vec![1; 4], 4).expect("fits");
    cache.insert(2, vec![2; 4], 4).expect("fits");
    cache.insert(3, vec![3; 4], 4).expect("fits");
    assert_eq!(cache.resident_bytes(), 3 * charge(4));
    assert_eq!(cache.get(&1), Some(&vec![1; 4]));
    cache.insert(4, vec![4; 4], 4).expect("fits after eviction");
    assert!(
        cache.get(&2).is_none(),
        "least recently used entry was evicted"
    );
    assert!(cache.get(&1).is_some());
    assert!(cache.get(&3).is_some());
    assert!(cache.get(&4).is_some());
    assert_eq!(cache.len(), 3);
    assert_eq!(cache.resident_bytes(), 3 * charge(4));
}

#[test]
fn replacing_a_key_accounts_only_the_new_charge() {
    let mut cache = ByteLru::<u64, Vec<u8>>::new(charge(64));
    cache.insert(7, vec![0; 16], 16).expect("fits");
    assert_eq!(cache.resident_bytes(), charge(16));
    cache.insert(7, vec![0; 64], 64).expect("replacement fits");
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.resident_bytes(), charge(64));
    assert_eq!(cache.get(&7).map(Vec::len), Some(64));
}

#[test]
fn oversized_entries_are_returned_uncached_without_evicting_residents() {
    let mut cache = ByteLru::<u64, Vec<u8>>::new(charge(8) + charge(8));
    cache.insert(1, vec![1; 8], 8).expect("fits");
    let big = vec![9; 1024];
    assert_eq!(cache.insert(2, big.clone(), 1024), Err(big));
    assert!(
        cache.get(&1).is_some(),
        "residents survive an oversized insert"
    );
    assert_eq!(cache.len(), 1);
}

#[test]
fn resident_bytes_track_every_insert_and_eviction() {
    let mut cache = ByteLru::<u64, Vec<u8>>::new(2 * charge(100));
    cache.insert(1, vec![0; 100], 100).expect("fits");
    cache.insert(2, vec![0; 100], 100).expect("fits");
    assert_eq!(cache.resident_bytes(), 2 * charge(100));
    cache
        .insert(3, vec![0; 100], 100)
        .expect("fits after eviction");
    assert_eq!(cache.resident_bytes(), 2 * charge(100));
    assert_eq!(cache.len(), 2);
}
