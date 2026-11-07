//! No-fork checking: every server's executed sequence, ⊥ and blocked servers included, is a
//! prefix of one order (the "does not fork" goal of pp. 29–30). This is neither Def 1.4 Strong
//! Safety (equality among useful servers) nor Def 1.5 Monotonicity (per server over time); a
//! lagging useful server and a server whose sequence shrinks both pass it.

use crate::smr::observe::canonical::CanonicalOrder;
use protocol::chunked::SealedMark;
use protocol::compact::Entry;
use protocol::shared_state::{ExecSeq, ExecutedView};

/// An executed sequence in any of its owned, borrowed, or chunked forms.
pub trait EntrySeq {
    fn entry_count(&self) -> usize;
    fn entries(&self) -> impl Iterator<Item = &Entry>;
}

impl EntrySeq for [Entry] {
    fn entry_count(&self) -> usize {
        self.len()
    }
    fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.iter()
    }
}

impl EntrySeq for Vec<Entry> {
    fn entry_count(&self) -> usize {
        self.len()
    }
    fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.iter()
    }
}

impl EntrySeq for ExecSeq {
    fn entry_count(&self) -> usize {
        self.len()
    }
    fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.iter()
    }
}

impl<S: EntrySeq + ?Sized> EntrySeq for &S {
    fn entry_count(&self) -> usize {
        (**self).entry_count()
    }
    fn entries(&self) -> impl Iterator<Item = &Entry> {
        (**self).entries()
    }
}

/// Reference no-fork check: all executed sequences are prefixes of the longest.
pub fn prefixes_consistent<S: EntrySeq>(seqs: &[S]) -> bool {
    let Some(longest) = seqs.iter().max_by_key(|s| s.entry_count()) else {
        return true;
    };
    seqs.iter()
        .all(|s| longest.entries().take(s.entry_count()).eq(s.entries()))
}

/// Incremental `prefixes_consistent`. Alg 5's S_i := S_j (p. 26) can rewrite a
/// sequence anywhere, so only a prefix the node provably still holds is skipped.
pub struct PrefixChecker {
    canonical: CanonicalOrder,
    verified: Vec<Verified>,
}

/// What the last check verified of one node: everything from `offset` on.
#[derive(Default)]
struct Verified {
    offset: u64,
    mark: Option<SealedMark<Entry>>,
}

impl Verified {
    /// First position of `seq` not known to be verified.
    fn recheck_from(&self, seq: ExecutedView<'_>) -> u64 {
        match &self.mark {
            Some(mark) if seq.offset() >= self.offset && seq.holds(mark) => {
                mark.end().max(seq.offset())
            }
            _ => seq.offset(),
        }
    }
}

impl PrefixChecker {
    pub fn new(n: usize) -> Self {
        Self {
            canonical: CanonicalOrder::new(),
            verified: (0..n).map(|_| Verified::default()).collect(),
        }
    }

    /// Verified canonical length; monotone.
    pub fn canonical_len(&self) -> usize {
        self.canonical.len() as usize
    }

    pub(in crate::smr) fn canonical(&self) -> &CanonicalOrder {
        &self.canonical
    }

    /// Extends reach without a verdict; re-running `check_round` could change which assert fires.
    pub(in crate::smr) fn absorb(&mut self, longest: ExecutedView<'_>) {
        let reach = self.canonical.len();
        if longest.logical_len() <= reach {
            return;
        }
        assert!(
            longest.offset() <= reach,
            "the boundary's longest executed sequence starts at {} — past the \
             verified order ({reach}), so its suffix cannot be attached to it",
            longest.offset()
        );
        self.canonical.extend_from_iter(longest.iter_from(reach));
    }

    /// Only the observer release line may call this.
    pub(in crate::smr) fn release_to(&mut self, upto: u64) {
        self.canonical.release_to(upto);
    }

    /// Verify one round's snapshots; false = a fork (caller latches). A shrink is re-verified, not flagged.
    pub fn check_round(&mut self, executed: &[ExecutedView<'_>], frontier: u64) -> bool {
        for (i, seq) in executed.iter().enumerate() {
            let len = seq.logical_len();
            let offset = seq.offset();
            debug_assert!(
                offset <= frontier && offset <= len,
                "node {i}: offset {offset} is above the frontier {frontier} or its own length {len}"
            );
            // Hard asserts: both bound a subtraction below.
            assert!(
                offset <= self.canonical.len(),
                "node {i}: offset {offset} is past the canonical order ({})",
                self.canonical.len()
            );
            assert!(
                offset >= self.canonical.start(),
                "node {i}: offset {offset} is below the retained canonical \
                 order (start {}) — the shrink path re-verifies from the \
                 offset, so the observer released a position this node can \
                 still re-present",
                self.canonical.start()
            );
            let start = self.verified[i].recheck_from(*seq);
            let overlap = len.min(self.canonical.len());
            if start < overlap && !seq.eq_slice_from(start, self.canonical.slice(start, overlap)) {
                return false;
            }
            if len > self.canonical.len() {
                let reach = self.canonical.len();
                self.canonical.extend_from_iter(seq.iter_from(reach));
            }
            self.verified[i] = Verified {
                offset,
                mark: seq.last_sealed(),
            };
        }
        true
    }
}
