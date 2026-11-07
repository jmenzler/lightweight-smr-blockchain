//! The log L_i as one value: its entries plus the carried index that sorts them.

use crate::chunked::ChunkSeq;
use crate::compact::{Entry, Timed};
use crate::merge::{MergedIndex, count_sealed, is_sorted_perm, sorted_perm};
use std::sync::Arc;

/// A non-⊥ log L_i; ⊥ is `None` in an `Option<Log>`.
///
/// Invariants: `perm` lists every position of `entries` exactly once, in entry-sorted order
/// (`merge::is_sorted_perm`); the two halves are built and replaced together, never apart;
/// and a `Log` changes in place only through copy-on-write (`strip_front`), so a clone
/// held by an observer keeps its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    entries: Arc<ChunkSeq<Timed>>,
    perm: Arc<Vec<u32>>,
}

impl Log {
    /// Sorts the entries once to build the index.
    pub fn from_entries(entries: Vec<Timed>) -> Log {
        let entries = ChunkSeq::from_vec(entries);
        let perm = sorted_perm(&entries);
        Log {
            entries: Arc::new(entries),
            perm: Arc::new(perm),
        }
    }

    /// Sorts the entries once to build the index.
    pub fn from_seq(entries: Arc<ChunkSeq<Timed>>) -> Log {
        let perm = Arc::new(sorted_perm(&entries));
        Log { entries, perm }
    }

    /// Freezes a log whose index the caller kept sorted.
    pub(crate) fn sealed(entries: ChunkSeq<Timed>, perm: Vec<u32>) -> Log {
        count_sealed(entries.len());
        Log {
            entries: Arc::new(entries),
            perm: Arc::new(perm),
        }
    }

    pub fn entries(&self) -> &Arc<ChunkSeq<Timed>> {
        &self.entries
    }

    /// Positions of `entries` in entry-sorted order; order among equal entries is unspecified.
    pub fn perm(&self) -> &Arc<Vec<u32>> {
        &self.perm
    }

    /// Whether `perm` sorts `entries` (invariant 1). A full re-check that reads only.
    pub fn is_consistent(&self) -> bool {
        is_sorted_perm(&self.entries, &self.perm)
    }

    /// Removes the first `n` entries and appends `x_d` if that empties the log. Copy-on-write:
    /// a clone an observer still holds keeps the unstripped content.
    pub(crate) fn strip_front(&mut self, n: usize, x_d: Timed) {
        assert!(
            n <= self.entries.len(),
            "cannot strip {n} of {} entries",
            self.entries.len()
        );
        let entries = Arc::make_mut(&mut self.entries);
        let perm = Arc::make_mut(&mut self.perm);
        entries.drop_front(n);
        let stripped = n as u32;
        perm.retain(|&place| place >= stripped);
        for place in perm.iter_mut() {
            *place -= stripped;
        }
        push_if_empty(entries, perm, x_d);
        #[cfg(debug_assertions)]
        let kept = (entries.len(), perm.len());
        perm.shrink_to_fit();
        #[cfg(debug_assertions)]
        assert_eq!(
            (entries.len(), perm.len()),
            kept,
            "giving the boundary strip's capacity back changed a length"
        );
        debug_assert!(
            is_sorted_perm(entries, perm),
            "the boundary strip left an index that does not sort its own log"
        );
    }
}

/// Appends the dummy command x_d, and its index slot, to an emptied log.
fn push_if_empty(entries: &mut ChunkSeq<Timed>, perm: &mut Vec<u32>, x_d: Timed) {
    if entries.is_empty() {
        entries.push(x_d);
        perm.push(0);
    }
}

/// L_i while Alg 5 step 4 builds it: owned entries plus the index the merge left.
///
/// Invariant: unless `index_stale`, `index` sorts `entries`. A rewrite of an entry sets
/// `index_stale`, and `remove_prefix` then rebuilds the index by a full sort instead of
/// shifting it.
pub struct MergedLog {
    entries: ChunkSeq<Timed>,
    index: MergedIndex,
    index_stale: bool,
}

impl MergedLog {
    pub(crate) fn new(entries: ChunkSeq<Timed>, index: MergedIndex) -> Self {
        MergedLog {
            entries,
            index,
            index_stale: false,
        }
    }

    pub fn entries(&self) -> &ChunkSeq<Timed> {
        &self.entries
    }

    /// Hands `rule` the entries and their sorted multiset; `rule` returns whether it rewrote any.
    pub fn rewrite(&mut self, rule: impl FnOnce(&mut ChunkSeq<Timed>, &[Entry]) -> bool) {
        // `present()` folds the index here, so `into_perm()` later counts no second fold.
        if rule(&mut self.entries, self.index.present()) {
            self.index_stale = true;
        }
    }

    /// Removes the first `n` entries, appends `x_d` if that empties the log, and seals it.
    pub fn remove_prefix(mut self, n: usize, x_d: Timed) -> Log {
        assert!(
            n <= self.entries.len(),
            "cannot remove {n} of {} entries",
            self.entries.len()
        );
        self.entries.drop_front(n);
        let mut perm: Vec<u32> = if self.index_stale {
            sorted_perm(&self.entries)
        } else {
            let n = n as u32;
            let full = self.index.into_perm();
            let mut perm = Vec::with_capacity(full.len() - n as usize);
            perm.extend(
                full.into_iter()
                    .filter(|&place| place >= n)
                    .map(|place| place - n),
            );
            perm
        };
        push_if_empty(&mut self.entries, &mut perm, x_d);
        debug_assert!(
            is_sorted_perm(&self.entries, &perm),
            "the merge left an index that does not sort its own log"
        );
        Log::sealed(self.entries, perm)
    }
}
