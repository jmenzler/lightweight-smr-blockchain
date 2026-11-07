//! Read-only views a driver or observer takes of a compact node, and memory upkeep; no box step runs here.

use super::{CompactNode, Log, OutOfSnOrder, SharedState, Timed};
use crate::chunked::ChunkSeq;
use crate::support::shared_state::{CLONE_SITE_COMPACT_COW_FORGET, cow_state};
use std::sync::Arc;

impl CompactNode {
    pub fn shared_state(&self) -> &SharedState {
        &self.s_i
    }

    pub fn state_arc(&self) -> &Arc<SharedState> {
        &self.s_i
    }

    pub fn repeat_skips(&self) -> u64 {
        self.repeat_skips
    }

    /// The commit the execute-once state machine refused; a driver ends the run on it.
    pub fn sn_order_violation(&self) -> Option<OutOfSnOrder> {
        self.sn_order_violation
    }

    /// Drops executed history below `upto`; S_i's logical content is unchanged.
    pub fn forget_committed_prefix(&mut self, upto: u64) {
        if self.s_i.forget_is_noop(upto) {
            return;
        }
        cow_state(&mut self.s_i, CLONE_SITE_COMPACT_COW_FORGET).forget_committed_prefix(upto);
    }

    pub fn log_seq(&self) -> Option<&ChunkSeq<Timed>> {
        self.l_i.as_ref().map(|l| &**l.entries())
    }

    pub fn log_entries(&self) -> Option<Vec<Timed>> {
        self.log_seq().map(ChunkSeq::to_vec)
    }

    pub fn log_arc(&self) -> Option<&Arc<ChunkSeq<Timed>>> {
        self.l_i.as_ref().map(Log::entries)
    }

    pub fn log_perm_arc(&self) -> Option<&Log> {
        self.l_i.as_ref()
    }
}
