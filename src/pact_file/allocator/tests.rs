use super::{Extent, ExtentAllocator};

#[test]
fn extent_is_reused_only_after_both_root_slots_release_it() {
    let mut allocator = ExtentAllocator::new(4096);
    let extent = allocator.allocate(8192, 1).expect("first extent");
    allocator.retire(extent, 2).expect("retire extent");
    assert_ne!(allocator.allocate(8192, 2).expect("generation 2"), extent);
    assert_ne!(allocator.allocate(8192, 3).expect("generation 3"), extent);
    allocator
        .release_before_generation(3)
        .expect("release retired");
    assert_eq!(allocator.allocate(8192, 4).expect("reuse extent"), extent);
}

#[test]
fn allocator_rejects_overlapping_live_extents() {
    let live = vec![
        Extent {
            offset: 4096,
            len: 8192,
        },
        Extent {
            offset: 8192,
            len: 4096,
        },
    ];
    assert!(ExtentAllocator::from_parts(16_384, live, vec![], vec![]).is_err());
}

#[test]
fn best_fit_splits_and_adjacent_free_extents_coalesce() {
    let mut allocator = ExtentAllocator::from_parts(
        32_768,
        vec![],
        vec![
            Extent {
                offset: 4096,
                len: 4096,
            },
            Extent {
                offset: 8192,
                len: 4096,
            },
            Extent {
                offset: 16_384,
                len: 8192,
            },
        ],
        vec![],
    )
    .expect("allocator");

    let first = allocator.allocate(6144, 1).expect("best fit");
    assert_eq!(
        first,
        Extent {
            offset: 4096,
            len: 6144,
        }
    );
    let second = allocator.allocate(2048, 1).expect("split remainder");
    assert_eq!(second.offset, 10_240);
}
