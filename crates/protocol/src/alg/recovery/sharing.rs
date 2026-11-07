//! Memory machinery: sharing the median's log handles, interning checkpoints and states
//! across nodes, and §4 forgetting.

use super::{Checkpoint, RecoveryNode};
use crate::certificates::ServerCertState;
use crate::compact::{ClientCommand, SharedState, Timed};
use crate::log::Log;
use crate::merge::{
    Chosen, StampTie, UnionStamp, common_prefix_len, is_sorted_perm, merge_onto_median,
    order_and_pick_median, union_is_noop,
};
use crate::support::shared_state::{CLONE_SITE_RECOVERY_COW_FORGET, cow_state};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static STATE_INTERN_TRIED: AtomicU64 = AtomicU64::new(0);
static STATE_INTERN_HIT: AtomicU64 = AtomicU64::new(0);

pub fn state_intern_counts() -> (u64, u64) {
    (
        STATE_INTERN_HIT.load(Ordering::Relaxed),
        STATE_INTERN_TRIED.load(Ordering::Relaxed),
    )
}

impl RecoveryNode {
    /// Hold `cp` in place of an own checkpoint the caller proved equal on (S, P, W).
    pub fn share_checkpoint(&mut self, cp: &Arc<Checkpoint>) {
        if Arc::ptr_eq(&self.c_i, cp) {
            return;
        }
        // Always-on: the caller's proof is a 64-bit digest, and a collision would be silent.
        assert_eq!(
            self.c_i.w, cp.w,
            "share_checkpoint across windows: the caller's equality proof does not cover W"
        );
        assert_eq!(
            self.c_i.s.logical_len(),
            cp.s.logical_len(),
            "share_checkpoint over states of unequal length: digest collision or a wrong key"
        );
        assert_eq!(
            cp.certs.is_some(),
            self.c_i.certs.is_some(),
            "share_checkpoint across the §5 gate: the fleet's certificate shape is a caller \
             convention this type does not enforce, and interning must not silently drop the \
             §5 layer"
        );
        assert!(
            self.c_i.p == cp.p,
            "share_checkpoint over unequal P: digest collision or a wrong key"
        );
        // A Null bumps sn(c) without extending S, so equal length does not pin the sn map.
        assert!(
            cp.s.sn_iter().eq(self.c_i.s.sn_iter()),
            "share_checkpoint over unequal committed sequence numbers: digest collision or a \
             wrong key"
        );
        assert!(
            same_forest(self.c_i.certs.as_ref(), cp.certs.as_ref()),
            "share_checkpoint over unequal §5 forests: digest collision or a wrong key"
        );
        debug_assert!(
            retained_entries_agree(&self.c_i.s, &cp.s),
            "share_checkpoint over states whose retained entries differ: digest collision or a \
             wrong key"
        );
        self.c_i = Arc::clone(cp);
    }

    pub fn forget_committed_prefix(&mut self, upto: u64) {
        if self.s_i.forget_is_noop(upto) {
            return;
        }
        cow_state(&mut self.s_i, CLONE_SITE_RECOVERY_COW_FORGET).forget_committed_prefix(upto);
    }

    /// As [`Self::end_window`], installing a peer's checkpoint for a proven-equal class.
    pub fn end_window_shared(&mut self, next_window: u64, round: u64, cp: &Arc<Checkpoint>) {
        assert_eq!(
            cp.w, next_window,
            "end_window_shared with a checkpoint of another window"
        );
        assert_eq!(
            cp.certs.is_some(),
            self.c_i.certs.is_some(),
            "end_window_shared across the §5 gate: the fleet's certificate shape is a caller \
             convention this type does not enforce, and a shared mint must not silently drop \
             the §5 layer"
        );
        self.between_windows(next_window, round, Some(cp));
        assert!(
            self.l_i.is_some(),
            "a ⊥ node mints nothing and cannot belong to a class"
        );
    }

    /// C_i := `cp`, a peer's checkpoint the caller proved equal to (S_i, P_i, W'). Also
    /// swaps S_i for the peer's state allocation when the two are equal in representation.
    pub(super) fn hold_equal_checkpoint(
        &mut self,
        cp: &Arc<Checkpoint>,
        p_i: &[Timed],
    ) -> Arc<Checkpoint> {
        assert!(
            cp.p.as_deref() == Some(p_i),
            "end_window_shared over an unequal P: the class key did not prove what it claimed"
        );
        assert_eq!(
            cp.s.logical_len(),
            self.s_i.logical_len(),
            "end_window_shared over unequal committed lengths: the class key did not prove \
             what it claimed"
        );
        // A Null bumps sn(c) without extending S, so equal length does not pin the sn map.
        assert!(
            cp.s.sn_iter().eq(self.s_i.sn_iter()),
            "end_window_shared over unequal committed sequence numbers: the class key did not \
             prove what it claimed"
        );
        debug_assert!(
            retained_entries_agree(&self.s_i, &cp.s),
            "end_window_shared over states whose retained entries differ: the class key did not \
             prove what it claimed"
        );
        debug_assert!(
            same_forest(self.c_i.attest_committed().as_ref(), cp.certs.as_ref()),
            "end_window_shared over unequal §5 forests: the class key did not prove what it \
             claimed"
        );
        // Intern only on representation equality; `PartialEq` would panic on differing offsets.
        STATE_INTERN_TRIED.fetch_add(1, Ordering::Relaxed);
        if self.s_i.content_eq(&cp.s) {
            STATE_INTERN_HIT.fetch_add(1, Ordering::Relaxed);
            self.s_i = Arc::clone(&cp.s);
        }
        Arc::clone(cp)
    }
}

/// Both layers absent, or both present over the same committed leaves.
fn same_forest(a: Option<&ServerCertState>, b: Option<&ServerCertState>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.forest().len() == b.forest().len() && a.forest().roots() == b.forest().roots()
        }
        _ => false,
    }
}

/// The entries both states still retain are the same; equal-length states differ in what they forgot.
fn retained_entries_agree(a: &SharedState, b: &SharedState) -> bool {
    let from = a.executed_offset().max(b.executed_offset());
    a.executed()
        .iter_from(from)
        .eq(b.executed().iter_from(from))
}

/// L_i := L'_i ∘ L̄ over the drawn logs M (Alg 3 step 4). When L̄ adds nothing, the median's
/// own handles are shared instead of copied.
pub(super) fn median_union(
    logs: &[&Log],
    m: &[usize],
    appends: &[(ClientCommand, u64)],
    ell: usize,
    stamp_tie: StampTie,
    union_stamp: UnionStamp,
) -> Log {
    let mut chosen: Vec<Chosen<'_>> = m
        .iter()
        .map(|&i| Chosen {
            log: logs[i].entries(),
            perm: &logs[i].perm()[..],
            origin: i as u32,
        })
        .collect();
    let cp = common_prefix_len(&chosen);
    let at = order_and_pick_median(&mut chosen, ell, cp, stamp_tie);
    if union_is_noop(&chosen, cp, appends) {
        // Sound only because every log mutation site is copy-on-write.
        let median = logs[chosen[at].origin as usize];
        debug_assert!(
            &**median.entries() == chosen[at].log,
            "the origin index does not name the median's own reply"
        );
        // Must be the median's own perm: equal-content logs may carry different perms.
        debug_assert_eq!(
            &median.perm()[..],
            chosen[at].perm,
            "the origin index does not name the median's carried index"
        );
        return median.clone();
    }
    let (log, index) = merge_onto_median(&chosen, at, cp, appends, union_stamp);
    let perm = index.into_perm();
    debug_assert!(
        is_sorted_perm(&log, &perm),
        "the merge left an index that does not sort its own log"
    );
    Log::sealed(log, perm)
}
