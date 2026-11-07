//! Per-server committed-sequence oracles over the executed sequence behind S_i (pp. 35–36):
//! Def 1.5 Monotonicity and Def 1.4 Strong Safety. Both sit beside the no-fork latch
//! `safety_ok`, which checks neither.
//!
//! A position a server forgot reads as the canonical order's, as everywhere in the observer:
//! servers forget only while the no-fork latch holds, so that is what they held there.

use crate::smr::observe::canonical::{CanonicalOrder, extend, view_digest_upto};
use protocol::chunked::SealedMark;
use protocol::compact::Entry;
use protocol::shared_state::ExecutedView;

pub(in crate::smr) struct CommittedOracle {
    seen: Vec<Seen>,
    retractions: u64,
    rewrites: u64,
    first_retraction: Option<usize>,
    strong: Tally,
    strong_boundary: Tally,
}

/// One server's executed sequence at its last observation.
struct Seen {
    len: u64,
    /// Position-anchored digest of the whole sequence, `CanonicalOrder::digest_at`'s chain.
    chain: u64,
    mark: Option<SealedMark<Entry>>,
    /// The entries in `[tail_from, len)`: past the last sealed chunk, or all retained without one.
    tail_from: u64,
    tail: Vec<Entry>,
}

/// Violating rounds, each counted once.
#[derive(Default)]
struct Tally {
    rounds: u64,
    first: Option<usize>,
    last: Option<usize>,
}

impl Tally {
    fn record(&mut self, round: usize) {
        if self.last != Some(round) {
            self.rounds += 1;
            self.last = Some(round);
        }
        self.first.get_or_insert(round);
    }

    fn get(&self) -> (u64, Option<usize>) {
        (self.rounds, self.first)
    }
}

/// The useful servers of every Def 1.4 counter, compact and recovery alike: L_i ≠ ⊥ after the
/// round's step (recovery: after the between-window pass when one runs) and not blocked in that
/// round, read at the round's end. Def 1.4 leaves the instant open; the paper reads "useful" at
/// the round's start for §§2–3 (pp. 10, 19) and as L_i ≠ ⊥ alone in §6 (p. 35).
pub(in crate::smr) fn useful_at_round_end(
    holds_log: impl Iterator<Item = bool>,
    mask: &[bool],
) -> Vec<bool> {
    holds_log
        .zip(mask)
        .map(|(held, &blocked)| held && !blocked)
        .collect()
}

impl Seen {
    fn genesis() -> Self {
        Seen {
            len: 0,
            chain: crate::digest::FNV_BASIS,
            mark: None,
            tail_from: 0,
            tail: Vec::new(),
        }
    }

    fn refresh(&mut self, view: ExecutedView<'_>, chain: u64) {
        let mark = view.last_sealed();
        self.tail_from = mark
            .as_ref()
            .map_or(view.offset(), SealedMark::end)
            .max(view.offset());
        self.len = view.logical_len();
        self.chain = chain;
        self.mark = mark;
        self.tail.clear();
        self.tail.extend(view.iter_from(self.tail_from).cloned());
    }

    /// Whether `view`, no shorter than this, holds every position this held.
    fn extended_by(&self, view: ExecutedView<'_>, canon: &CanonicalOrder) -> bool {
        let shares_prefix = match &self.mark {
            // A held sealed chunk agrees with this on every position below its end.
            Some(mark) => view.holds(mark),
            None => view.offset() >= self.tail_from,
        };
        if !shares_prefix {
            return view_digest_upto(canon, view, self.len) == self.chain;
        }
        let from = self.tail_from.max(view.offset());
        from >= self.len || view.eq_slice_from(from, &self.tail[(from - self.tail_from) as usize..])
    }

    /// The chain of `view`, which extends this.
    fn chain_of_extension(&self, view: ExecutedView<'_>, canon: &CanonicalOrder) -> u64 {
        if view.offset() > self.len {
            return view_digest_upto(canon, view, view.logical_len());
        }
        let mut chain = self.chain;
        view.for_each_range(self.len, view.logical_len(), |at, entry| {
            chain = extend(chain, at, entry);
        });
        chain
    }
}

impl CommittedOracle {
    pub(in crate::smr) fn new(n: usize) -> Self {
        CommittedOracle {
            seen: (0..n).map(|_| Seen::genesis()).collect(),
            retractions: 0,
            rewrites: 0,
            first_retraction: None,
            strong: Tally::default(),
            strong_boundary: Tally::default(),
        }
    }

    /// Def 1.5 (p. 8): one observation of every server. A sequence that is not an extension of
    /// the server's previous one is a retraction; a rewrite is one that did not get shorter.
    pub(in crate::smr) fn observe(
        &mut self,
        executed: &[ExecutedView<'_>],
        canon: &CanonicalOrder,
        round: usize,
    ) {
        for (seen, &view) in self.seen.iter_mut().zip(executed) {
            let shrank = view.logical_len() < seen.len;
            let extends = !shrank && seen.extended_by(view, canon);
            let chain = if extends {
                seen.chain_of_extension(view, canon)
            } else {
                self.retractions += 1;
                self.rewrites += u64::from(!shrank);
                self.first_retraction.get_or_insert(round);
                view_digest_upto(canon, view, view.logical_len())
            };
            seen.refresh(view, chain);
        }
    }

    /// Def 1.4 (p. 7) at the last observation: `None` when no server is useful, else whether
    /// every useful server holds the same executed sequence.
    fn agreement(&self, useful: &[bool]) -> Option<bool> {
        let mut members = self
            .seen
            .iter()
            .zip(useful)
            .filter(|&(_, &u)| u)
            .map(|(seen, _)| (seen.len, seen.chain));
        let first = members.next()?;
        Some(members.all(|m| m == first))
    }

    /// Def 1.4 Strong Safety over [`useful_at_round_end`]: a disagreement counts the round, and
    /// in a window-end round also the window-end tally. Returns [`Self::agreement`].
    pub(in crate::smr) fn check_strong_safety(
        &mut self,
        useful: &[bool],
        round: usize,
        window_end: bool,
    ) -> Option<bool> {
        let verdict = self.agreement(useful);
        if verdict == Some(false) {
            self.strong.record(round);
            if window_end {
                self.strong_boundary.record(round);
            }
        }
        verdict
    }

    /// (retractions, the rewrites among them), counted per server and observation.
    pub(in crate::smr) fn retractions(&self) -> (u64, u64) {
        (self.retractions, self.rewrites)
    }

    pub(in crate::smr) fn first_retraction_round(&self) -> Option<usize> {
        self.first_retraction
    }

    /// (rounds with a violation, the first one).
    pub(in crate::smr) fn strong_safety(&self) -> (u64, Option<usize>) {
        self.strong.get()
    }

    pub(in crate::smr) fn strong_safety_at_boundaries(&self) -> (u64, Option<usize>) {
        self.strong_boundary.get()
    }
}

#[cfg(test)]
mod tests;
