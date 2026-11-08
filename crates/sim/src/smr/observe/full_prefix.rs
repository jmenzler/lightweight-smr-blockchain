//! Live-only Lemma 3.8 T_E landmark (p. 24) for compact and recovery, where a server's
//! sequence is its executed part followed by its log: the first round at whose end every
//! non-⊥ server's sequence begins with one identical prefix through x, compared as entries.
//!
//! The prefix checker has verified every executed part as a prefix of the canonical order
//! before this runs, so executed parts agree by construction and only logs are compared.

use crate::smr::observe::canonical::CanonicalOrder;
use crate::smr::observe::entry_index::EntryIndex;
use crate::smr::tracker::CommandTracker;
use protocol::chunked::ChunkSeq;
use protocol::compact::{Entry, Timed};
use rustc_hash::FxHashSet;

/// One non-⊥ server's sequence: `executed` canonical entries, then its log.
pub(in crate::smr) struct FullSeq<'a> {
    pub(in crate::smr) executed: u64,
    pub(in crate::smr) log: &'a ChunkSeq<Timed>,
}

impl FullSeq<'_> {
    fn end(&self) -> u64 {
        self.executed + self.log.len() as u64
    }

    fn key(&self) -> (u64, *const ChunkSeq<Timed>) {
        (self.executed, self.log)
    }
}

/// The non-⊥ servers' sequences from a round's post-step logs and executed lengths.
pub(in crate::smr) fn full_seqs<'a>(
    post_logs: &[Option<&'a ChunkSeq<Timed>>],
    executed_lens: &[usize],
) -> Vec<FullSeq<'a>> {
    post_logs
        .iter()
        .zip(executed_lens)
        .filter_map(|(log, &executed)| {
            log.map(|log| FullSeq {
                executed: executed as u64,
                log,
            })
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::smr) struct CommonPrefix {
    /// Shared by every sequence (Lemma 3.8's prefix, the one T_E reads).
    pub(in crate::smr) len: u64,
    /// Shared by every sequence that contradicts the reference somewhere: a sequence that is a
    /// strict prefix of the reference lags behind it but disagrees with nobody, so it is left out.
    pub(in crate::smr) contested: u64,
    /// The longest sequence among those with the longest executed part, ties by lowest index;
    /// a lagging reference would make every other server look in agreement.
    pub(in crate::smr) reference: usize,
}

/// The oldest unsettled age past each of the two common prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct UnsettledAge {
    /// `e5_probe`'s `max_unsettled_age` over the non-⊥ servers.
    pub full: u64,
    /// The same past the contested prefix: lagging servers left out.
    pub contested: u64,
}

/// Both common prefixes; `None` without a sequence.
pub(in crate::smr) fn common_prefix(
    canonical: &CanonicalOrder,
    seqs: &[FullSeq<'_>],
) -> Option<CommonPrefix> {
    let reference = (0..seqs.len())
        .rev()
        .max_by_key(|&i| (seqs[i].executed, seqs[i].end()))?;
    let r = &seqs[reference];
    // Bounded by the contested minimum: a lagging sequence that ends below it is still read to
    // its end, so the full minimum is exact too.
    let mut contested = r.end();
    let mut lagging = r.end();
    // Servers holding one log handle at one executed length have one sequence.
    let mut seen: FxHashSet<(u64, *const ChunkSeq<Timed>)> = FxHashSet::default();
    seen.insert(r.key());
    for s in seqs {
        if contested == 0 {
            break;
        }
        if !seen.insert(s.key()) {
            continue;
        }
        let agreed = agreeing_len(canonical, s, r, contested);
        if agreed == s.end() {
            lagging = lagging.min(agreed);
        } else {
            contested = agreed;
        }
    }
    Some(CommonPrefix {
        len: contested.min(lagging),
        contested,
        reference,
    })
}

/// How far `s` agrees with the reference `r`, looking no further than `bound` ≤ `r.end()`.
fn agreeing_len(canonical: &CanonicalOrder, s: &FullSeq<'_>, r: &FullSeq<'_>, bound: u64) -> u64 {
    debug_assert!(s.executed <= r.executed && bound <= r.end());
    let bound = bound.min(s.end());
    let mut p = s.executed;
    let against_canonical = r.executed.min(bound);
    if p < against_canonical {
        let theirs = canonical.slice(p, against_canonical);
        for (mine, want) in s.log.iter().zip(theirs) {
            if mine.entry != *want {
                return p;
            }
            p += 1;
        }
    }
    if p >= bound {
        return bound;
    }
    if s.executed == r.executed {
        // Same frame: sealed chunks held by the same allocation are equal entry for entry.
        let shared = s.log.shared_prefix_chunks(r.log);
        p += (s.log.live_len_of_sealed_prefix(shared) as u64).min(bound - p);
    }
    let mine = s.log.iter_at((p - s.executed) as usize);
    let theirs = r.log.iter_at((p - r.executed) as usize);
    for (a, b) in mine.zip(theirs).take((bound - p) as usize) {
        if a.entry != b.entry {
            return p;
        }
        p += 1;
    }
    p
}

/// `e5_probe`'s `max_unsettled_age` past `lcp`: the oldest stamp among log entries at or past it, 0 if none.
pub(in crate::smr) fn oldest_unsettled_age(round: u64, lcp: u64, seqs: &[FullSeq<'_>]) -> u64 {
    let mut seen: FxHashSet<(u64, *const ChunkSeq<Timed>)> = FxHashSet::default();
    let mut oldest = 0;
    for s in seqs.iter().filter(|s| seen.insert(s.key())) {
        let from = lcp.saturating_sub(s.executed) as usize;
        for t in s.log.iter_at(from) {
            oldest = oldest.max(round.saturating_sub(t.round));
        }
    }
    oldest
}

/// Dates `full_prefix_fixed_round` once per command over the full prefix and returns the round's
/// oldest unsettled ages; `None` when no server holds a log. `floor` is the canonical index's
/// release line.
pub(in crate::smr) fn observe_full_prefix(
    round: usize,
    canonical: &CanonicalOrder,
    canon_index: &EntryIndex,
    floor: u64,
    seqs: &[FullSeq<'_>],
    trackers: &mut [CommandTracker],
) -> Option<UnsettledAge> {
    let prefix = common_prefix(canonical, seqs)?;
    let r = &seqs[prefix.reference];
    let in_log: FxHashSet<&Entry> = r
        .log
        .iter()
        .take(prefix.len.saturating_sub(r.executed) as usize)
        .map(|t| &t.entry)
        .collect();
    for t in trackers
        .iter_mut()
        .filter(|t| t.full_prefix_fixed_round.is_none() && round >= t.injection_round)
    {
        let entry = Entry::Cmd(t.cc);
        // Below the release line every server executed it, so a command there was dated already.
        let executed_inside = canon_index
            .get_from(&entry, floor)
            .is_some_and(|at| u64::from(at) < prefix.len);
        if executed_inside || in_log.contains(&entry) {
            t.full_prefix_fixed_round = Some(round);
        }
    }
    Some(UnsettledAge {
        full: oldest_unsettled_age(round as u64, prefix.len, seqs),
        contested: oldest_unsettled_age(round as u64, prefix.contested, seqs),
    })
}

#[cfg(test)]
mod tests;
