use super::Budget;

#[test]
fn leases_reserve_checked_bytes_and_release_on_drop() {
    let budget = Budget::new(100);
    let first = budget.try_reserve(60).expect("60 fits");
    assert_eq!(budget.used(), 60);
    assert!(budget.try_reserve(50).is_none(), "110 exceeds the limit");
    assert_eq!(budget.used(), 60, "a refused reservation changes nothing");
    let second = budget.try_reserve(40).expect("exactly the limit fits");
    assert_eq!(budget.used(), 100);
    assert_eq!(budget.peak(), 100);
    drop(first);
    assert_eq!(budget.used(), 40);
    drop(second);
    assert_eq!(budget.used(), 0);
    assert_eq!(budget.peak(), 100, "peak is retained after release");
    assert_eq!(budget.limit(), 100);
}

#[test]
fn zero_budget_refuses_everything_but_tracked_reservations_still_account() {
    let budget = Budget::new(0);
    assert!(budget.try_reserve(1).is_none());
    assert!(
        budget.try_reserve(0).is_some(),
        "an empty reservation is free"
    );
    let tracked = budget.reserve_tracked(7);
    assert_eq!(tracked.bytes(), 7);
    assert_eq!(budget.used(), 7);
    assert_eq!(budget.peak(), 7);
    drop(tracked);
    assert_eq!(budget.used(), 0);
}

#[test]
fn reservation_arithmetic_is_checked() {
    let budget = Budget::new(usize::MAX);
    let _big = budget.try_reserve(usize::MAX - 1).expect("fits");
    assert!(
        budget.try_reserve(2).is_none(),
        "an overflowing sum is refused, not wrapped"
    );
}
