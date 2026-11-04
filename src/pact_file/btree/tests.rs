use std::collections::BTreeMap;

use super::{CowBTree, PAGE_BYTES, PAGE_HEADER_BYTES, PageReader, PageStore};
use crate::PactErrorKind;

fn key(value: u64) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

#[test]
fn bulk_load_builds_a_dense_tree_without_intermediate_cow_pages() {
    let mut pages = PageStore::default();
    let entries = (0..512)
        .map(|index| {
            (
                format!("component/{index:04}").into_bytes(),
                format!("value-{index:04}").into_bytes(),
            )
        })
        .collect::<Vec<_>>();

    let tree = CowBTree::bulk_load(&mut pages, entries.clone()).expect("bulk load");

    assert!(
        pages.page_count() < 20,
        "bulk tree used {} pages",
        pages.page_count()
    );
    for (key, expected) in entries.iter().step_by(31) {
        assert_eq!(tree.get(&pages, key).expect("get"), Some(expected.clone()));
    }
    assert_eq!(
        tree.range(&pages, b"component/0000", b"component/9999")
            .expect("range")
            .len(),
        entries.len()
    );
}

#[test]
fn copy_on_write_roots_preserve_old_values_and_replace_duplicates() {
    let mut pages = PageStore::default();
    let empty = CowBTree::default();
    let (first, _) = empty
        .insert(&mut pages, key(1), b"one".to_vec())
        .expect("first insert");
    let (second, _) = first
        .insert(&mut pages, key(1), b"updated".to_vec())
        .expect("replacement");

    assert_eq!(
        first.get(&pages, &key(1)).expect("old read"),
        Some(b"one".to_vec())
    );
    assert_eq!(
        second.get(&pages, &key(1)).expect("new read"),
        Some(b"updated".to_vec())
    );
}

#[test]
fn insertion_externalizes_an_inline_value_when_no_two_way_split_fits() {
    let mut pages = PageStore::default();
    let mut tree = CowBTree::default();
    let left = b"a-left-key-01".to_vec();
    let middle = b"m-mid-key--02".to_vec();
    let right = b"z-right-key03".to_vec();

    (tree, _) = tree
        .insert(&mut pages, left.clone(), vec![b'l'; 278])
        .expect("insert left value");
    (tree, _) = tree
        .insert(&mut pages, right.clone(), vec![b'r'; 278])
        .expect("insert right value");
    (tree, _) = tree
        .insert(&mut pages, middle.clone(), vec![b'm'; 428])
        .expect("externalize the middle value");

    assert_eq!(
        tree.get(&pages, &left).expect("read left"),
        Some(vec![b'l'; 278])
    );
    assert_eq!(
        tree.get(&pages, &middle).expect("read middle"),
        Some(vec![b'm'; 428])
    );
    assert_eq!(
        tree.get(&pages, &right).expect("read right"),
        Some(vec![b'r'; 278])
    );
}

#[test]
fn splits_and_range_scans_match_btree_map_with_logarithmic_page_writes() {
    let mut pages = PageStore::default();
    let mut tree = CowBTree::default();
    let mut expected = BTreeMap::new();
    let mut state = 0x5eed_u64;
    for index in 0..2_000_u64 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let item_key = state % 3_000;
        let value = format!("value-{index}").into_bytes();
        let (next, stats) = tree
            .insert(&mut pages, key(item_key), value.clone())
            .expect("random insert");
        assert!(
            stats.pages_written <= 16,
            "wrote {} pages",
            stats.pages_written
        );
        tree = next;
        expected.insert(key(item_key), value);
    }

    let actual = tree
        .range(&pages, &key(500), &key(1_500))
        .expect("range scan");
    let expected = expected
        .range(key(500)..key(1_500))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(pages.page_bytes(), pages.page_count() * PAGE_BYTES);
}

#[test]
fn corrupt_page_checksum_returns_typed_error() {
    let mut pages = PageStore::default();
    let (tree, _) = CowBTree::default()
        .insert(&mut pages, key(1), b"one".to_vec())
        .expect("insert");
    let root = tree.root_address().expect("root");
    pages.pages.get_mut(&root).expect("root page")[PAGE_HEADER_BYTES] ^= 0xff;

    let error = tree.get(&pages, &key(1)).expect_err("checksum error");
    assert_eq!(error.kind(), PactErrorKind::CorruptFile);
}

#[test]
fn copy_on_write_deletes_merge_pages_and_preserve_old_roots() {
    let mut pages = PageStore::default();
    let mut tree = CowBTree::default();
    for item in 0..400_u64 {
        (tree, _) = tree
            .insert(&mut pages, key(item), vec![b'x'; 24])
            .expect("insert for delete");
    }
    let original = tree.clone();
    for item in 0..400_u64 {
        let (next, stats, removed) = tree.delete(&mut pages, &key(item)).expect("delete");
        assert_eq!(removed, Some(vec![b'x'; 24]));
        assert!(stats.pages_written <= 32);
        tree = next;
    }

    assert!(tree.root_address().is_none());
    assert_eq!(
        original.get(&pages, &key(200)).expect("old root read"),
        Some(vec![b'x'; 24])
    );
}

#[test]
fn randomized_insert_delete_parity_matches_btree_map() {
    let mut pages = PageStore::default();
    let mut tree = CowBTree::default();
    let mut expected = BTreeMap::new();
    let mut state = 0xdecafbad_u64;
    for operation in 0..2_000_u64 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let item = state % 700;
        if state & 3 == 0 {
            let expected_removed = expected.remove(&key(item));
            let (next, _, removed) = tree.delete(&mut pages, &key(item)).expect("delete");
            assert_eq!(removed, expected_removed);
            tree = next;
        } else {
            let value = operation.to_le_bytes().to_vec();
            expected.insert(key(item), value.clone());
            (tree, _) = tree.insert(&mut pages, key(item), value).expect("insert");
        }
    }
    let actual = tree.range(&pages, &key(0), &key(701)).expect("range");
    assert_eq!(actual, expected.into_iter().collect::<Vec<_>>());
}

#[test]
fn limited_ranges_read_only_tree_paths_and_window_leaves() {
    let mut pages = PageStore::default();
    let mut tree = CowBTree::default();
    for item in 0..10_000_u64 {
        (tree, _) = tree
            .insert(&mut pages, key(item), item.to_le_bytes().to_vec())
            .expect("insert");
    }
    let counted = CountingPages {
        pages: &pages,
        reads: std::cell::Cell::new(0),
    };
    let forward = tree
        .range_limit(&counted, &key(5_000), &key(9_000), 11, false)
        .expect("forward range");
    assert_eq!(forward.len(), 11);
    assert_eq!(
        forward.first().map(|(key, _)| key.clone()),
        Some(key(5_000))
    );
    assert!(counted.reads.get() < 30);

    counted.reads.set(0);
    let reverse = tree
        .range_limit(&counted, &key(1_000), &key(5_001), 11, true)
        .expect("reverse range");
    assert_eq!(reverse.len(), 11);
    assert_eq!(
        reverse.first().map(|(key, _)| key.clone()),
        Some(key(5_000))
    );
    assert!(counted.reads.get() < 30);
}

#[test]
fn narrow_ranges_read_only_overlapping_subtrees() {
    let mut pages = PageStore::default();
    let entries = (0..10_000_u64)
        .map(|item| (key(item), item.to_le_bytes().to_vec()))
        .collect::<Vec<_>>();
    let tree = CowBTree::bulk_load(&mut pages, entries).expect("bulk load");
    let counted = CountingPages {
        pages: &pages,
        reads: std::cell::Cell::new(0),
    };

    let actual = tree
        .range(&counted, &key(5_000), &key(5_005))
        .expect("narrow range");

    assert_eq!(actual.len(), 5);
    assert_eq!(actual.first().map(|(key, _)| key.clone()), Some(key(5_000)));
    assert_eq!(actual.last().map(|(key, _)| key.clone()), Some(key(5_004)));
    assert!(
        counted.reads.get() < 30,
        "narrow range read {} pages",
        counted.reads.get()
    );
}

#[test]
fn dense_range_windows_use_order_statistics_after_deletes() {
    let mut pages = PageStore::default();
    let mut tree = CowBTree::default();
    let mut expected = BTreeMap::new();
    for item in 0..10_000_u64 {
        let value = item.to_le_bytes().to_vec();
        (tree, _) = tree
            .insert(&mut pages, key(item), value.clone())
            .expect("insert");
        expected.insert(key(item), value);
    }
    for item in (1_000..9_000_u64).step_by(7) {
        (tree, _, _) = tree.delete(&mut pages, &key(item)).expect("delete");
        expected.remove(&key(item));
    }

    let expected = expected
        .range(key(1_000)..key(9_000))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    let counted = CountingPages {
        pages: &pages,
        reads: std::cell::Cell::new(0),
    };
    let (total, window) = tree
        .range_window(&counted, &key(1_000), &key(9_000), 4_000, 13)
        .expect("dense range window");

    assert_eq!(total, expected.len());
    assert_eq!(window, expected[4_000..4_013]);
    assert!(
        counted.reads.get() < 80,
        "dense window read {} pages",
        counted.reads.get()
    );
}

#[test]
fn oversized_values_use_overflow_pages_and_round_trip_through_all_reads() {
    let mut pages = PageStore::default();
    let value = (0..24_000_u32)
        .flat_map(|number| number.to_le_bytes())
        .collect::<Vec<_>>();
    let (tree, stats) = CowBTree::default()
        .insert(&mut pages, key(7), value.clone())
        .expect("insert oversized value");

    assert!(stats.pages_written > 1);
    assert_eq!(tree.get(&pages, &key(7)).expect("get"), Some(value.clone()));
    assert_eq!(
        tree.range(&pages, &key(0), &key(8)).expect("range"),
        vec![(key(7), value.clone())]
    );
    let (empty, _, removed) = tree.delete(&mut pages, &key(7)).expect("delete");
    assert_eq!(removed, Some(value));
    assert!(empty.root_address().is_none());
}

struct CountingPages<'a> {
    pages: &'a PageStore,
    reads: std::cell::Cell<u64>,
}

impl PageReader for CountingPages<'_> {
    fn read_page(&self, address: u64) -> crate::PactResult<[u8; PAGE_BYTES]> {
        self.reads.set(self.reads.get() + 1);
        self.pages.read_page(address)
    }
}

fn leaf_depths(pages: &impl PageReader, address: u64, depth: usize, out: &mut Vec<usize>) {
    match super::read_node(pages, address).expect("node") {
        super::Node::Leaf { .. } => out.push(depth),
        super::Node::Internal { children, .. } => {
            for child in children {
                leaf_depths(pages, child, depth + 1, out);
            }
        }
        super::Node::Overflow { .. } => panic!("structural walk reached an overflow page"),
    }
}

#[test]
fn deletes_that_empty_subtrees_keep_every_leaf_at_one_depth() {
    let mut pages = PageStore::default();
    let mut mirror = BTreeMap::new();
    let mut tree = CowBTree::from_root(None);
    for value in 0..3_000_u64 {
        (tree, _) = tree
            .insert(&mut pages, key(value), vec![value as u8; 96])
            .expect("insert");
        mirror.insert(key(value), vec![value as u8; 96]);
    }

    let doomed = (0..3_000_u64)
        .filter(|value| value % 400 != 200)
        .collect::<Vec<_>>();
    for (step, value) in doomed.iter().enumerate() {
        let (next, _, removed) = tree.delete(&mut pages, &key(*value)).expect("delete");
        assert!(removed.is_some(), "key {value} was present");
        tree = next;
        mirror.remove(&key(*value));
        if step % 97 == 0 || step + 1 == doomed.len() {
            let mut depths = Vec::new();
            leaf_depths(&pages, tree.root_address().expect("root"), 0, &mut depths);
            depths.dedup();
            assert_eq!(
                depths.len(),
                1,
                "leaves at several depths after deleting {value}"
            );
            let entries = tree
                .range(&pages, &[], &[0xff])
                .expect("range")
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            assert_eq!(entries, mirror, "after deleting {value}");
        }
    }
    assert_eq!(mirror.len(), 7);
}
