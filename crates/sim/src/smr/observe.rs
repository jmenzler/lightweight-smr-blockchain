//! Read-only measurement: nothing here draws from the run's RNG or mutates a node.
//!
//! Two outputs do reach the world, by design: recovery's `executed_round` gates the non-ack
//! client's resends and frees its pool slot, and the cursor floor plus the safety latch bound
//! lean-mode forgetting. Neither may change what the protocol does;
//! `tests/regression/observer_inertness.rs` pins that.

pub(super) mod accounting;
pub(super) mod boundary_skip;
pub(super) mod canon_cursor;
pub(super) mod canonical;
pub(super) mod census_count;
pub(super) mod committed;
pub(super) mod detail;
pub(super) mod entry_index;
pub(super) mod exec_dup;
pub(super) mod fleet_wedge;
pub(super) mod log_index;
pub(super) mod round_metrics;
pub(super) mod safety;
pub(super) mod spill;
