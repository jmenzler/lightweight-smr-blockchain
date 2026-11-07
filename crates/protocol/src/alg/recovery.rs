//! Algorithm 6, the recovery protocol (p. 31), on top of Algorithm 3 (p. 20). Each bullet of
//! either box is marked `// Alg N: "<its opening words>"`, in the order of its box.

use crate::Config;
use crate::certificates::ServerCertState;
use crate::compact::{
    ClientCommand, Entry, OutOfSnOrder, RepeatedCommit, SharedState, Timed, Triage,
};
use crate::log::Log;
use crate::merge::{StampTie, UnionStamp};
pub use crate::support::checkpoint::RecoveryPrefixViolation;
use crate::support::checkpoint::{
    aged_prefix, assert_prefix, execute_prefix, log_from_prefix, strip_prefix, validate_prefix,
};
use rand::Rng;
use rand::seq::index::sample;
use std::sync::Arc;

mod observe;
mod sharing;
#[cfg(test)]
mod tests;

use sharing::median_union;
pub use sharing::state_intern_counts;

/// R_i: server i's willingness to roll back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RState {
    NoReset,
    Reset,
    Bot,
}

/// What a node does at a boundary where P is not a prefix of L_i; Alg 6 has no branch for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefixMismatch {
    /// Panic: the paper rules the case out w.h.p. (Lemma 6.7). Only the no-reset path can reach
    /// it; a same-W checkpoint fork that a rollback commits is a population check for drivers.
    #[default]
    Abort,
    /// Leave the boundary undone and wait to adopt a newer peer checkpoint.
    SkipBoundary,
}

/// C_i = (S, P, W); P is ⊥ initially, which is not the empty prefix.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub s: Arc<SharedState>,
    pub p: Option<Vec<Timed>>,
    pub w: u64,
    /// §5 certificate state; `None` runs Algorithm 6 as printed.
    pub certs: Option<ServerCertState>,
}

/// Representation equality: P is compared with its round stamps, which an adopting node takes
/// over into L_i and ages by. The paper's same-W identity (p. 35) reads P as commands only.
/// Panics through `SharedState::eq` on equal-length states with different forget lines, so
/// states held by different nodes are compared with `content_eq` or a fork signature.
impl PartialEq for Checkpoint {
    fn eq(&self, other: &Self) -> bool {
        self.s == other.s && self.p == other.p && self.w == other.w
    }
}

impl Eq for Checkpoint {}

impl Checkpoint {
    /// The §5 certificate state the next checkpoint carries: this one's plus P, now committed.
    fn attest_committed(&self) -> Option<ServerCertState> {
        // EXTENSION: [EXT-2] §5 certificates ride the checkpoint; P is attested when it commits (Lemma 6.9)
        self.certs.as_ref().map(|held| {
            let mut certs = held.clone();
            let committed = self.p.iter().flatten();
            for t in committed.filter(|t| matches!(t.entry, Entry::Cmd(_))) {
                certs.append(&t.entry);
            }
            certs
        })
    }
}

/// A reply from server j with R_j ≠ ⊥: (C_j, R_j), plus L_j iff it holds a log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReply {
    pub l_j: Option<Log>,
    pub c_j: Arc<Checkpoint>,
    pub r_j: RState,
}

/// Algorithm 6, the recovery protocol.
pub struct RecoveryNode {
    /// S_i.
    s_i: Arc<SharedState>,
    /// L_i; `None` is ⊥.
    l_i: Option<Log>,
    r_i: RState,
    c_i: Arc<Checkpoint>,
    cfg: Config,
    /// T, the length of a T-window.
    t: u64,
    stamp_tie: StampTie,
    union_stamp: UnionStamp,
    prefix_mismatch: PrefixMismatch,
    repeated_commit: RepeatedCommit,
    /// Entries the execute-once state machine left out; each marks an upstream agreement failure.
    repeat_skips: u64,
    /// This node never commits C_i's skipped P; cleared by adoption. C_i is still served to peers.
    stale_checkpoint: bool,
    /// A boundary commit the execute-once state machine refused; the node is final from then on.
    sn_order_violation: Option<OutOfSnOrder>,
}

impl RecoveryNode {
    /// S_i = s₀, L_i = [x₀], R_i = no-reset, C_i = (s₀, ⊥, 0). `certs` mounts the §5 layer.
    pub fn new(cfg: Config, t: u64, certs: bool) -> Self {
        cfg.assert_valid();
        assert!(t >= 1, "T-window length must be at least 1");
        RecoveryNode {
            s_i: Arc::new(SharedState::default()),
            l_i: Some(Log::from_entries(vec![Timed {
                entry: Entry::Nop(0),
                round: 0,
            }])),
            r_i: RState::NoReset,
            c_i: Arc::new(Checkpoint {
                s: Arc::new(SharedState::default()),
                p: None,
                w: 0,
                certs: certs.then(ServerCertState::new),
            }),
            cfg,
            t,
            stamp_tie: StampTie::IncludeRound,
            union_stamp: UnionStamp::FirstSighting,
            prefix_mismatch: PrefixMismatch::Abort,
            repeated_commit: RepeatedCommit::Execute,
            repeat_skips: 0,
            stale_checkpoint: false,
            sn_order_violation: None,
        }
    }

    pub fn with_prefix_mismatch(mut self, policy: PrefixMismatch) -> Self {
        self.prefix_mismatch = policy;
        self
    }

    /// Not combined with §5: the checkpoint would attest a skipped command a second time.
    pub fn with_repeated_commit(mut self, policy: RepeatedCommit) -> Self {
        assert!(
            policy == RepeatedCommit::Execute || self.c_i.certs.is_none(),
            "RepeatedCommit::Skip is not combined with the §5 certificate layer"
        );
        self.repeated_commit = policy;
        self
    }

    /// The boundary commit the execute-once state machine refused; a driver ends the run on it.
    pub fn sn_order_violation(&self) -> Option<OutOfSnOrder> {
        self.sn_order_violation
    }

    fn assert_not_refused(&self) {
        assert!(
            self.sn_order_violation.is_none(),
            "a node that refused an out-of-sn-order commit is not stepped again"
        );
    }

    pub fn with_merge_policy(mut self, stamp: StampTie, union: UnionStamp) -> Self {
        self.stamp_tie = stamp;
        self.union_stamp = union;
        self
    }

    // Alg 3: "for every command x received from a client that is not yet contained in L_i"
    pub fn wants_amplify(&self, x: &ClientCommand) -> bool {
        match &self.l_i {
            None => true,
            Some(l_i) => !l_i.entries().iter().any(|t| t.entry == Entry::Cmd(*x)),
        }
    }

    // Alg 5: "for every command x received from a client c"
    /// §6 subsumes the §4 client sequence numbers in the state machine (p. 30).
    // DETERMINIZED: [DET-17] a ⊥ server ignores client commands (Alg 5's guard); Alg 3 alone has none
    pub fn on_client_command(&self, x: &ClientCommand) -> Triage {
        // Every step that replaces S_i or C_i keeps their sn maps equal, so this read is not a choice.
        debug_assert!(
            self.s_i.sn_iter().eq(self.c_i.s.sn_iter()),
            "S_i and C_i.S disagree on sn(c): a step replaced one without the other"
        );
        self.client_triage(x)
    }

    fn client_triage(&self, x: &ClientCommand) -> Triage {
        // sn(c) is "assumed to be included in S_i" (Alg 5 box, p. 26).
        let sn_c = self.s_i.sn_get(x.client).unwrap_or(0);
        // Alg 5: "if L_i ≠ ⊥, sn(x) = sn(c) + 1 and x ∉ L_i"
        if let Some(l_i) = &self.l_i
            && sn_c.checked_add(1) == Some(x.sn)
            && !l_i.entries().iter().any(|t| t.entry == Entry::Cmd(*x))
        {
            return Triage::Amplify;
        }
        // Alg 5: "if L_i ≠ ⊥ and sn(x) = sn(c)"
        if self.l_i.is_some() && x.sn == sn_c {
            return Triage::AckCommitted;
        }
        // Alg 5: "otherwise, i ignores x"
        Triage::Ignore
    }

    // Alg 6: "If a request was received from server j and R_i ≠ ⊥"
    // Alg 3: "if L_i ≠ ⊥ then for any log request received by server i from a server j"
    pub fn answer(&self) -> Option<RecoveryReply> {
        (self.r_i != RState::Bot).then(|| RecoveryReply {
            l_j: self.l_i.clone(),
            c_j: Arc::clone(&self.c_i),
            r_j: self.r_i,
        })
    }

    /// The random half of Alg 3's "server i chooses a subset M of size ℓ from the received
    /// logs"; `None` unless ℓ replies carry a log. Drivers draw for every node in ascending id.
    pub fn draw_reply_choice<R: Rng + ?Sized>(
        &self,
        replies: &[RecoveryReply],
        rng: &mut R,
    ) -> Option<Vec<usize>> {
        if replies.len() < self.cfg.ell {
            return None;
        }
        let bearing = replies.iter().filter(|r| r.l_j.is_some()).count();
        (bearing >= self.cfg.ell).then(|| sample(rng, bearing, self.cfg.ell).into_vec())
    }

    /// One round during a T-window.
    pub fn step<R: Rng + ?Sized>(
        &mut self,
        replies: &[RecoveryReply],
        appends: &[(ClientCommand, u64)],
        _round: u64,
        rng: &mut R,
    ) {
        let m = self.draw_reply_choice(replies, rng);
        self.step_chosen(replies, appends, m.as_deref());
    }

    /// As [`Self::step`] with M already drawn; draws nothing.
    pub fn step_chosen(
        &mut self,
        replies: &[RecoveryReply],
        appends: &[(ClientCommand, u64)],
        m: Option<&[usize]>,
    ) {
        self.assert_not_refused();
        // DETERMINIZED: [DET-7] the Alg 3 merge runs first; adoption below overwrites L_i
        self.extended_median_rule(replies, appends, m);
        if replies.len() >= self.cfg.ell {
            self.at_least_ell_replies(replies);
        } else {
            self.fewer_than_ell_replies();
        }
    }

    /// Algorithm 3's merge over the replies that carry a log.
    fn extended_median_rule(
        &mut self,
        replies: &[RecoveryReply],
        appends: &[(ClientCommand, u64)],
        m: Option<&[usize]>,
    ) {
        // DETERMINIZED: [DET-5] Alg 3 counts only log-bearing replies toward ℓ (Lemma 6.2 is one-way)
        let logs: Vec<&Log> = replies.iter().filter_map(|r| r.l_j.as_ref()).collect();
        assert_eq!(
            m.is_some(),
            logs.len() >= self.cfg.ell,
            "M must be drawn exactly when the log-bearing reply count reaches ell"
        );
        self.l_i = match m {
            // Alg 3: "if server i receives at least ℓ replies"
            // Alg 3: "server i chooses a subset M of size ℓ from the received logs"
            // Alg 3: "server i sets L_i := L'_i ∘ L̄"
            // DETERMINIZED: [DET-1] the median comparator reads the round stamp (stamp_tie)
            // DETERMINIZED: [DET-2] a repeated command keeps its first-seen stamp (union_stamp)
            // DETERMINIZED: [DET-12] L̄ is the sorted M in order, then the appends in the driver's order
            Some(m) => Some(median_union(
                &logs,
                m,
                appends,
                self.cfg.ell,
                self.stamp_tie,
                self.union_stamp,
            )),
            // Alg 3: "if server i receives less than ℓ logs"
            None => None,
        };
    }

    // Alg 6: "If at least ℓ replies were received"
    fn at_least_ell_replies(&mut self, replies: &[RecoveryReply]) {
        // Alg 6: "If no-reset was received in one reply"
        // DETERMINIZED: [DET-16] any no-reset reply wins, as in the box; Def 3.5 would subsample ℓ
        self.r_i = if replies.iter().any(|rep| rep.r_j == RState::NoReset) {
            RState::NoReset
        } else {
            RState::Reset
        };
        // Alg 6: "let C' = (S', P', W') be the received checkpoint with largest W'"
        let c_prime = &replies[latest_checkpoint_index(replies)].c_j;
        // Alg 6: "If W' > W"
        if c_prime.w > self.c_i.w {
            self.s_i = Arc::clone(&c_prime.s);
            self.l_i = log_from_prefix(c_prime.p.as_deref());
            self.c_i = Arc::clone(c_prime);
            self.stale_checkpoint = false;
        }
    }

    // Alg 6: "If less than ℓ replies were received"
    fn fewer_than_ell_replies(&mut self) {
        self.r_i = RState::Bot;
    }

    /// The "Between T-windows" steps; `next_window` is W'.
    pub fn end_window(&mut self, next_window: u64, round: u64) {
        self.between_windows(next_window, round, None);
    }

    /// The between-T-window steps, run in box order, each on the result of the one before.
    /// `equal_peer` is another node's checkpoint that the caller proved equal to the C_i this
    /// node would make; the node then holds that one instead of a copy.
    fn between_windows(
        &mut self,
        next_window: u64,
        round: u64,
        equal_peer: Option<&Arc<Checkpoint>>,
    ) {
        self.assert_not_refused();
        // DEVIATION: [DEV-3] opt-in: a P that does not prefix L_i skips boundaries, L_i := ⊥, until adoption
        if self.skips_boundary() {
            self.stale_checkpoint = true;
            // Peers strip what they commit now; an unstripped L_i would gossip it back for a second commit.
            self.l_i = None;
        } else {
            // Alg 6: "If R_i = reset"
            if self.r_i == RState::Reset {
                self.s_i = Arc::clone(&self.c_i.s);
                self.l_i = log_from_prefix(self.c_i.p.as_deref());
            }
            // Alg 6: "If L_i ≠ ⊥"
            if let Some(l_i) = self.l_i.as_mut() {
                // Alg 6: "for current checkpoint C_i = (S, P, W), commit all commands in P on S_i and remove the prefix P from L_i"
                // P = ⊥ (the genesis checkpoint) commits nothing.
                let p = self.c_i.p.as_deref().unwrap_or(&[]);
                assert_prefix(p, l_i.entries(), round);
                // EXTENSION: [EXT-3] opt-in execute-once state machine at the boundary commit; an sn gap in P stops the boundary here
                match execute_prefix(&mut self.s_i, p, self.repeated_commit) {
                    Ok(skipped) => self.repeat_skips += skipped,
                    Err(gap) => {
                        self.sn_order_violation = Some(gap);
                        return;
                    }
                }
                strip_prefix(l_i, p.len(), round);
                // Alg 6: "make new checkpoint C_i := (S_i, P_i, W')"
                let p_i = aged_prefix(&l_i.entries().to_vec(), round, self.t);
                self.c_i = match equal_peer {
                    None => Arc::new(Checkpoint {
                        s: Arc::clone(&self.s_i),
                        p: Some(p_i),
                        w: next_window,
                        certs: self.c_i.attest_committed(),
                    }),
                    Some(peer) => self.hold_equal_checkpoint(peer, &p_i),
                };
                // Alg 6: "set R_i := no-reset"
                self.r_i = RState::NoReset;
            }
        }
        // Alg 6: "If L_i = ⊥"
        if self.l_i.is_none() {
            self.r_i = RState::Reset;
        }
    }

    /// Under `SkipBoundary`: C_i's P was skipped before, or does not prefix a non-reset L_i.
    fn skips_boundary(&self) -> bool {
        if self.prefix_mismatch != PrefixMismatch::SkipBoundary {
            return false;
        }
        if self.stale_checkpoint {
            return true;
        }
        let Some(l_i) = self.l_i.as_ref().filter(|_| self.r_i != RState::Reset) else {
            return false;
        };
        let p = self.c_i.p.as_deref().unwrap_or(&[]);
        validate_prefix(p, l_i.entries().len(), l_i.entries().iter()).is_err()
    }
}

/// The reply holding C' = (S', P', W'), the received checkpoint with the largest window number W'.
pub fn latest_checkpoint_index(replies: &[RecoveryReply]) -> usize {
    // DETERMINIZED: [DET-6] ties on the largest W' go to the LAST reply (max_by_key)
    replies
        .iter()
        .enumerate()
        .max_by_key(|(_, rep)| rep.c_j.w)
        .map(|(index, _)| index)
        .expect("at least ell replies")
}
