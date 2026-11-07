//! Algorithm 5, the compact (k,ℓ)-median rule (p. 26). Each bullet of the box is marked
//! `// Alg 5: "<its opening words>"`, in the order of the box.

use crate::Config;
use crate::chunked::ChunkSeq;
use crate::merge::{StampTie, UnionStamp, median_merge};
pub use crate::support::log::Log;
use crate::support::shared_state::{CLONE_SITE_COMPACT_COW_APPEND, cow_state};
pub use crate::support::shared_state::{OutOfSnOrder, RepeatedCommit, SharedState};
use rand::Rng;
use rand::seq::index::sample;
use std::collections::BTreeSet;
use std::sync::Arc;

pub type ClientId = u32;

/// Auto-minted client ids live at and above this base; scripted ids stay below.
pub const AUTO_CLIENT_BASE: ClientId = 1 << 31;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClientCommand {
    pub client: ClientId,
    pub sn: u64,
    pub op: u64,
}

/// A log slot: a client command, the null command ⊥, or a no-op filler (x₀, x_d).
// DETERMINIZED: [DET-10] the derived order is the paper's K order: Cmd < Null < Nop, commands by (client, sn, op)
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Entry {
    Cmd(ClientCommand),
    Null { client: ClientId, sn: u64 },
    Nop(u64),
}

impl Entry {
    /// The (client, sn) a command or ⊥ occupies; a no-op has none.
    pub fn client_sn(&self) -> Option<(ClientId, u64)> {
        match self {
            Entry::Cmd(c) => Some((c.client, c.sn)),
            Entry::Null { client, sn } => Some((*client, *sn)),
            Entry::Nop(_) => None,
        }
    }

    pub fn op(&self) -> Option<u64> {
        match self {
            Entry::Cmd(c) => Some(c.op),
            Entry::Null { .. } | Entry::Nop(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Timed {
    pub entry: Entry,
    pub round: u64,
}

pub type LogSeq = ChunkSeq<Timed, { crate::chunked::CHUNK }>;

/// A reply to a request: L_j, plus S_j iff the requester's bit b was 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactReply {
    pub l_j: Log,
    pub s_j: Option<Arc<SharedState>>,
}

impl CompactReply {
    pub fn from_log(l_j: Arc<ChunkSeq<Timed>>, s_j: Option<Arc<SharedState>>) -> Self {
        CompactReply {
            l_j: Log::from_seq(l_j),
            s_j,
        }
    }
}

/// What server i does with a command x received from a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Triage {
    /// Send σ log n append requests with x and the current round.
    Amplify,
    /// Inform the client that sn(x) has already been committed.
    AckCommitted,
    Ignore,
}

/// Algorithm 5, the compact (k,ℓ)-median rule (p. 26).
pub struct CompactNode {
    /// S_i, which includes sn(c) for every client c.
    s_i: Arc<SharedState>,
    /// L_i; `None` is ⊥.
    l_i: Option<Log>,
    cfg: Config,
    /// T: a command whose age reaches T is committed.
    t: u64,
    stamp_tie: StampTie,
    union_stamp: UnionStamp,
    repeated_commit: RepeatedCommit,
    /// Entries the execute-once state machine left out; each marks an upstream agreement failure.
    repeat_skips: u64,
    /// A commit the execute-once state machine refused; the node is final from then on.
    sn_order_violation: Option<OutOfSnOrder>,
}

impl CompactNode {
    /// S_i starts as s₀ and L_i as the seed command x₀.
    pub fn new(cfg: Config, t: u64) -> Self {
        cfg.assert_valid();
        CompactNode {
            s_i: Arc::new(SharedState::default()),
            l_i: Some(Log::from_entries(vec![Timed {
                entry: Entry::Nop(0),
                round: 0,
            }])),
            cfg,
            t,
            stamp_tie: StampTie::IncludeRound,
            union_stamp: UnionStamp::FirstSighting,
            repeated_commit: RepeatedCommit::Execute,
            repeat_skips: 0,
            sn_order_violation: None,
        }
    }

    pub fn with_repeated_commit(mut self, policy: RepeatedCommit) -> Self {
        self.repeated_commit = policy;
        self
    }

    pub fn with_merge_policy(mut self, stamp: StampTie, union: UnionStamp) -> Self {
        self.stamp_tie = stamp;
        self.union_stamp = union;
        self
    }

    /// sn(c), 0 before c's first commit.
    fn sn(&self, c: ClientId) -> u64 {
        self.s_i.sn_get(c).unwrap_or(0)
    }

    // Alg 5: "for every command x received from a client c"
    pub fn on_client_command(&self, x: &ClientCommand) -> Triage {
        let sn_c = self.sn(x.client);
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

    // Alg 5: "i sends k requests ... and attaches a bit b_i"
    pub fn b_i(&self) -> bool {
        self.l_i.is_none()
    }

    // Alg 5: "if L_i ≠ ⊥ then for any request received by i from some server j"
    pub fn answer(&self, b_j: bool) -> Option<CompactReply> {
        Some(CompactReply {
            l_j: self.l_i.clone()?,
            s_j: b_j.then(|| Arc::clone(&self.s_i)),
        })
    }

    /// The random half of "i chooses a subset M of ℓ of the replies"; `None` below ℓ replies.
    /// Drivers draw for every node in ascending id, then apply the steps in parallel.
    pub fn draw_reply_choice<R: Rng + ?Sized>(
        &self,
        reply_count: usize,
        rng: &mut R,
    ) -> Option<Vec<usize>> {
        (reply_count >= self.cfg.ell).then(|| sample(rng, reply_count, self.cfg.ell).into_vec())
    }

    /// One round after the replies arrived.
    pub fn step<R: Rng + ?Sized>(
        &mut self,
        replies: &[CompactReply],
        appends: &[(ClientCommand, u64)],
        round: u64,
        rng: &mut R,
    ) {
        let m = self.draw_reply_choice(replies.len(), rng);
        self.step_chosen(replies, appends, round, m.as_deref());
    }

    /// As [`Self::step`] with M already drawn; draws nothing.
    pub fn step_chosen(
        &mut self,
        replies: &[CompactReply],
        appends: &[(ClientCommand, u64)],
        round: u64,
        m: Option<&[usize]>,
    ) {
        assert!(
            self.sn_order_violation.is_none(),
            "a node that refused an out-of-sn-order commit is not stepped again"
        );
        let b_i = self.b_i();
        assert_eq!(
            m.is_some(),
            replies.len() >= self.cfg.ell,
            "M must be drawn exactly when the reply count reaches ell"
        );
        match m {
            Some(m) => self.at_least_ell_replies(replies, m, appends, round, b_i),
            None => self.fewer_than_ell_replies(replies, round, b_i),
        }
    }

    // Alg 5: "if i receives at least ℓ replies"
    fn at_least_ell_replies(
        &mut self,
        replies: &[CompactReply],
        m: &[usize],
        appends: &[(ClientCommand, u64)],
        round: u64,
        b_i: bool,
    ) {
        // Alg 5: "i chooses a subset M of ℓ of the replies uniformly at random"
        let m: Vec<&CompactReply> = m.iter().map(|&j| &replies[j]).collect();
        // Alg 5: "i sets L_i := L'_i ∘ L̄"
        // DETERMINIZED: [DET-1] the median comparator reads the round stamp (stamp_tie)
        // DETERMINIZED: [DET-2] a repeated command keeps its first-seen stamp (union_stamp)
        // DETERMINIZED: [DET-12] L̄ is the sorted M in order, then the appends in the driver's order
        let mut l_i = median_merge(
            m.iter().map(|r| &r.l_j),
            self.cfg.ell,
            appends,
            self.stamp_tie,
            self.union_stamp,
        );
        // Alg 5: "if the logs in L_i contain different commands from the same client with the same sequence number"
        l_i.rewrite(replace_duplicates_with_bot);
        // Alg 5: "if b_i = 1 then i picks any reply S_j"
        // DETERMINIZED: [DET-3] "any S_j in M" is the first drawn reply that carries a state
        if b_i && let Some(s_j) = m.iter().find_map(|r| r.s_j.as_ref()) {
            self.s_i = Arc::clone(s_j);
        }
        // Alg 5: "i determines the largest prefix P_i of L_i"
        let p_i = aged_prefix_len(l_i.entries().iter(), round, self.t);
        // Alg 5: "i commits the commands in P_i in the given order on S_i, removes P_i from L_i"
        if self.commit(l_i.entries(), p_i) {
            self.l_i = Some(l_i.remove_prefix(p_i, x_d(round)));
        }
    }

    // Alg 5: "if i receives less than ℓ replies"
    fn fewer_than_ell_replies(&mut self, replies: &[CompactReply], round: u64, b_i: bool) {
        // Alg 5: "i sets L_i := ⊥"
        self.l_i = None;
        // Alg 5: "if at least one reply is received then i picks any one of them"
        // DETERMINIZED: [DET-4] "any one" reply = the first; it gives both state and prefix
        let Some(r) = replies.first() else {
            return;
        };
        if b_i && let Some(s_j) = &r.s_j {
            self.s_i = Arc::clone(s_j);
        }
        let p_j = aged_prefix_len(r.l_j.entries().iter(), round, self.t);
        // DETERMINIZED: [DET-13] "the updates" include executing that reply's aged prefix, unfiltered
        self.commit(r.l_j.entries(), p_j);
    }

    /// Executes the first `p` entries of `log` on S_i in order; x₀ and x_d change nothing.
    /// Returns false iff the execute-once state machine refused the whole prefix.
    fn commit(&mut self, log: &ChunkSeq<Timed>, p: usize) -> bool {
        if log.iter().take(p).all(|t| matches!(t.entry, Entry::Nop(_))) {
            return true;
        }
        // EXTENSION: [EXT-3] an sn gap in P refuses the whole commit: no entry of P runs; an adopted S_j stays adopted
        if self.repeated_commit == RepeatedCommit::Skip
            && let Err(gap) = self
                .s_i
                .check_sn_order(log.iter().take(p).map(|t| &t.entry))
        {
            self.sn_order_violation = Some(gap);
            return false;
        }
        let s_i = cow_state(&mut self.s_i, CLONE_SITE_COMPACT_COW_APPEND);
        for t in log.iter().take(p) {
            // EXTENSION: [EXT-3] opt-in execute-once state machine, on both reply paths
            if !s_i.commit(&t.entry, self.repeated_commit) {
                self.repeat_skips += 1;
            }
        }
        true
    }
}

/// The dummy command x_d appended to an emptied log; it changes no state.
// DETERMINIZED: [DET-11] x_d is Nop(round) stamped with the current round, so it ages like a command and sorts above commands
pub(crate) fn x_d(round: u64) -> Timed {
    Timed {
        entry: Entry::Nop(round),
        round,
    }
}

/// Length of the longest prefix whose entries are all at least `t` rounds old.
pub(crate) fn aged_prefix_len<'a>(
    log: impl IntoIterator<Item = &'a Timed>,
    round: u64,
    t: u64,
) -> usize {
    log.into_iter()
        .take_while(|e| round.saturating_sub(e.round) >= t)
        .count()
}

/// Replaces every command whose (client, sn) carries two different commands with ⊥; returns whether any was replaced.
fn replace_duplicates_with_bot(log: &mut ChunkSeq<Timed>, sorted: &[Entry]) -> bool {
    debug_assert!(
        {
            let mut check: Vec<Entry> = log.iter().map(|t| t.entry.clone()).collect();
            check.sort_unstable();
            check == sorted
        },
        "sorted must be the log's entries as a sorted multiset"
    );
    let mut conflicting: BTreeSet<(ClientId, u64)> = BTreeSet::new();
    for pair in sorted.windows(2) {
        if let [Entry::Cmd(a), Entry::Cmd(b)] = pair
            && (a.client, a.sn) == (b.client, b.sn)
            && a.op != b.op
        {
            conflicting.insert((a.client, a.sn));
        }
    }
    if conflicting.is_empty() {
        return false;
    }
    let hits: Vec<usize> = log
        .iter()
        .enumerate()
        .filter_map(|(i, t)| match t.entry {
            Entry::Cmd(x) if conflicting.contains(&(x.client, x.sn)) => Some(i),
            _ => None,
        })
        .collect();
    for i in hits {
        let slot = log.make_mut_at(i);
        if let Entry::Cmd(x) = slot.entry {
            slot.entry = Entry::Null {
                client: x.client,
                sn: x.sn,
            };
        }
    }
    true
}

mod observe;

#[cfg(test)]
mod tests;
