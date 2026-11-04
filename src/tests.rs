#[path = "architecture_tests.rs"]
mod architecture;
#[path = "boundary_guard_tests.rs"]
mod boundary_guard;
#[path = "error_tests.rs"]
mod error;

pub(crate) mod instrumentation {
    use std::cell::Cell;

    thread_local! {
        pub(crate) static BRANCH_TRACE_COMPONENT_TREE_REPLAYS: Cell<usize> = const { Cell::new(0) };
        pub(crate) static LEDGER_HYDRATION_COUNT: Cell<usize> = const { Cell::new(0) };
        pub(crate) static PACT_TRANSACTION_DECODE_COUNT: Cell<usize> = const { Cell::new(0) };
        pub(crate) static HEAD_METADATA_SNAPSHOT_DECODE_COUNT: Cell<usize> = const { Cell::new(0) };
        pub(crate) static FILE_SELECTOR_FULL_SCAN_COUNT: Cell<usize> = const { Cell::new(0) };
        pub(crate) static PROJECTION_BUILD_COUNT: Cell<usize> = const { Cell::new(0) };
        pub(crate) static SELECTOR_FULL_INDEX_COUNT: Cell<usize> = const { Cell::new(0) };
        pub(crate) static TREE_EDIT_CALLS: Cell<usize> = const { Cell::new(0) };
        pub(crate) static POSITIONAL_REPLAY_TRANSACTIONS: Cell<usize> = const { Cell::new(0) };

        pub(crate) static FULL_HISTORY_LOADS: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn reset_full_history_loads() {
        FULL_HISTORY_LOADS.with(|count| count.set(0));
    }

    pub(crate) fn full_history_loads() -> usize {
        FULL_HISTORY_LOADS.with(Cell::get)
    }

    pub(crate) fn reset_positional_replay_transactions() {
        POSITIONAL_REPLAY_TRANSACTIONS.with(|count| count.set(0));
    }

    pub(crate) fn positional_replay_transactions() -> usize {
        POSITIONAL_REPLAY_TRANSACTIONS.with(Cell::get)
    }

    pub(crate) fn reset_branch_trace_component_tree_replay_count() {
        BRANCH_TRACE_COMPONENT_TREE_REPLAYS.with(|count| count.set(0));
    }

    pub(crate) fn branch_trace_component_tree_replay_count() -> usize {
        BRANCH_TRACE_COMPONENT_TREE_REPLAYS.with(Cell::get)
    }

    pub(crate) fn reset_projection_build_count() {
        PROJECTION_BUILD_COUNT.with(|count| count.set(0));
    }

    pub(crate) fn projection_build_count() -> usize {
        PROJECTION_BUILD_COUNT.with(Cell::get)
    }

    pub(crate) fn reset_ledger_hydration_count() {
        LEDGER_HYDRATION_COUNT.with(|count| count.set(0));
    }

    pub(crate) fn ledger_hydration_count() -> usize {
        LEDGER_HYDRATION_COUNT.with(Cell::get)
    }

    pub(crate) fn reset_pact_transaction_decode_count() {
        PACT_TRANSACTION_DECODE_COUNT.with(|count| count.set(0));
    }

    pub(crate) fn pact_transaction_decode_count() -> usize {
        PACT_TRANSACTION_DECODE_COUNT.with(Cell::get)
    }

    pub(crate) fn reset_head_metadata_snapshot_decode_count() {
        HEAD_METADATA_SNAPSHOT_DECODE_COUNT.with(|count| count.set(0));
    }

    pub(crate) fn head_metadata_snapshot_decode_count() -> usize {
        HEAD_METADATA_SNAPSHOT_DECODE_COUNT.with(Cell::get)
    }

    pub(crate) fn reset_file_selector_full_scan_count() {
        FILE_SELECTOR_FULL_SCAN_COUNT.with(|count| count.set(0));
    }

    pub(crate) fn file_selector_full_scan_count() -> usize {
        FILE_SELECTOR_FULL_SCAN_COUNT.with(Cell::get)
    }

    pub(crate) fn reset_selector_full_index_count() {
        SELECTOR_FULL_INDEX_COUNT.with(|count| count.set(0));
    }

    pub(crate) fn selector_full_index_count() -> usize {
        SELECTOR_FULL_INDEX_COUNT.with(Cell::get)
    }
}
