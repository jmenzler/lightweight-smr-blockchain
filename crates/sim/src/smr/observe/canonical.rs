//! Observer's canonical committed order and its position-anchored digest chain.

use crate::digest::{FNV_BASIS, fold, fold_entry};
use protocol::compact::{Entry, SharedState, Timed};
use protocol::recovery::Checkpoint;
use protocol::shared_state::ExecutedView;
use rustc_hash::FxHashMap;
use std::marker::PhantomData;

pub(in crate::smr) fn extend(chain: u64, at: u64, entry: &Entry) -> u64 {
    fold_entry(fold(chain, &at.to_le_bytes()), entry)
}

pub(in crate::smr) fn timed_digest(entries: &[Timed]) -> u64 {
    entries.iter().fold(FNV_BASIS, |h, t| {
        fold_entry(fold(h, &t.round.to_le_bytes()), &t.entry)
    })
}

fn command_digest(entries: &[Timed]) -> u64 {
    entries
        .iter()
        .fold(FNV_BASIS, |h, t| fold_entry(h, &t.entry))
}

pub(in crate::smr) struct CanonicalOrder {
    start: u64,
    entries: Vec<Entry>,
    digests: Vec<u64>,
    /// Everything released, so tests check each scan the release cut short against a full one.
    #[cfg(debug_assertions)]
    released: Vec<Entry>,
}

impl CanonicalOrder {
    pub(in crate::smr) fn new() -> Self {
        CanonicalOrder {
            start: 0,
            entries: Vec::new(),
            digests: vec![FNV_BASIS],
            #[cfg(debug_assertions)]
            released: Vec::new(),
        }
    }

    pub(in crate::smr) fn len(&self) -> u64 {
        self.start + self.entries.len() as u64
    }

    pub(in crate::smr) fn start(&self) -> u64 {
        self.start
    }

    pub(in crate::smr) fn retained(&self) -> &[Entry] {
        &self.entries
    }

    fn index(&self, at: u64, what: &str) -> usize {
        assert!(
            at >= self.start,
            "canonical position {at} is below the released prefix (start {}) \
             while reading {what} — the observer released a position something \
             still reads, which the release line exists to rule out",
            self.start
        );
        assert!(
            at <= self.len(),
            "canonical position {at} is past the verified order ({}) while \
             reading {what}",
            self.len()
        );
        (at - self.start) as usize
    }

    pub(in crate::smr) fn slice(&self, from: u64, to: u64) -> &[Entry] {
        let (lo, hi) = (self.index(from, "a slice"), self.index(to, "a slice"));
        &self.entries[lo..hi]
    }

    pub(in crate::smr) fn digest_at(&self, at: u64) -> u64 {
        self.digests[self.index(at, "a digest")]
    }

    pub(in crate::smr) fn extend_from_slice(&mut self, entries: &[Entry]) {
        self.extend_from_iter(entries.iter());
    }

    pub(in crate::smr) fn extend_from_iter<'a>(
        &mut self,
        entries: impl Iterator<Item = &'a Entry>,
    ) {
        for entry in entries {
            let at = self.len();
            let chain = extend(*self.digests.last().expect("seeded"), at, entry);
            self.entries.push(entry.clone());
            self.digests.push(chain);
        }
    }

    pub(in crate::smr) fn release_to(&mut self, upto: u64) {
        let upto = upto.min(self.len());
        if upto <= self.start {
            return;
        }
        let drop = (upto - self.start) as usize;
        let _dropped = self.entries.drain(..drop);
        #[cfg(debug_assertions)]
        self.released.extend(_dropped);
        self.digests.drain(..drop);
        self.start = upto;
    }

    pub(in crate::smr) fn heap_bytes(&self) -> u64 {
        (self.entries.capacity() * size_of::<Entry>() + self.digests.capacity() * size_of::<u64>())
            as u64
    }

    #[cfg(debug_assertions)]
    fn unreleased(&self, from: u64, to: u64) -> impl Iterator<Item = &Entry> {
        self.released
            .iter()
            .chain(&self.entries)
            .take(to as usize)
            .skip(from as usize)
    }

    #[cfg(test)]
    pub(in crate::smr) fn from_entries(entries: &[Entry]) -> Self {
        let mut order = CanonicalOrder::new();
        order.extend_from_slice(entries);
        order
    }
}

/// Chain value at `upto`: the state's own entries above its offset, the observer's value below.
pub(in crate::smr) fn state_digest_upto(
    canon: &CanonicalOrder,
    state: &SharedState,
    upto: u64,
) -> u64 {
    view_digest_upto(canon, state.executed(), upto)
}

/// [`state_digest_upto`] of a bare executed view.
pub(in crate::smr) fn view_digest_upto(
    canon: &CanonicalOrder,
    ex: ExecutedView<'_>,
    upto: u64,
) -> u64 {
    let base = upto.min(ex.offset());
    let mut chain = canon.digest_at(base);
    ex.for_each_range(ex.offset(), upto, |at, entry| {
        chain = extend(chain, at, entry);
    });
    chain
}

pub(in crate::smr) fn state_digest(canon: &CanonicalOrder, state: &SharedState) -> u64 {
    state_digest_upto(canon, state, state.logical_len())
}

/// The paper's checkpoint identity (S, P, W), p. 35, with P a sequence of commands (p. 31):
/// P's round stamps are not part of it. `Checkpoint::eq` asserts on differing forget offsets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::smr) struct ForkSignature {
    executed_len: u64,
    executed: u64,
    sn: u64,
    // None is ⊥, distinct from the empty prefix.
    pre: Option<(usize, u64)>,
}

impl ForkSignature {
    #[cfg(test)]
    pub(in crate::smr) fn of(canon: &CanonicalOrder, checkpoint: &Checkpoint) -> Self {
        Self::with_executed(checkpoint, state_digest(canon, &checkpoint.s))
    }

    fn with_executed(checkpoint: &Checkpoint, executed: u64) -> Self {
        ForkSignature {
            executed_len: checkpoint.s.logical_len(),
            executed,
            sn: crate::digest::fnv64_sn(checkpoint.s.sn_iter()),
            pre: checkpoint.p.as_ref().map(|p| (p.len(), command_digest(p))),
        }
    }

    pub(in crate::smr) fn executed_len(&self) -> u64 {
        self.executed_len
    }

    pub(in crate::smr) fn executed(&self) -> u64 {
        self.executed
    }

    pub(in crate::smr) fn pre_len(&self) -> Option<usize> {
        self.pre.map(|(len, _)| len)
    }

    pub(in crate::smr) fn extends(
        &self,
        canon: &CanonicalOrder,
        checkpoint: &Checkpoint,
        upto: u64,
        mark: u64,
    ) -> bool {
        self.executed_len >= upto && state_digest_upto(canon, &checkpoint.s, upto) == mark
    }
}

/// [`state_digest`] and [`ForkSignature::of`] against one canonical order, once per allocation.
/// The `'a` borrows keep every keyed allocation alive, so no address is reused while cached.
pub(in crate::smr) struct DigestCache<'a> {
    canon: &'a CanonicalOrder,
    states: FxHashMap<*const SharedState, u64>,
    signatures: FxHashMap<*const Checkpoint, ForkSignature>,
    pinned: PhantomData<&'a Checkpoint>,
}

impl<'a> DigestCache<'a> {
    pub(in crate::smr) fn new(canon: &'a CanonicalOrder) -> Self {
        DigestCache {
            canon,
            states: FxHashMap::default(),
            signatures: FxHashMap::default(),
            pinned: PhantomData,
        }
    }

    pub(in crate::smr) fn canon(&self) -> &'a CanonicalOrder {
        self.canon
    }

    pub(in crate::smr) fn state(&mut self, state: &'a SharedState) -> u64 {
        let canon = self.canon;
        *self
            .states
            .entry(std::ptr::from_ref(state))
            .or_insert_with(|| state_digest(canon, state))
    }

    pub(in crate::smr) fn signature(&mut self, checkpoint: &'a Checkpoint) -> ForkSignature {
        let key = std::ptr::from_ref(checkpoint);
        if let Some(&signature) = self.signatures.get(&key) {
            return signature;
        }
        let signature = ForkSignature::with_executed(checkpoint, self.state(&checkpoint.s));
        self.signatures.insert(key, signature);
        signature
    }

    /// (state digests, signatures) computed so far.
    #[cfg(test)]
    pub(in crate::smr) fn computed(&self) -> (usize, usize) {
        (self.states.len(), self.signatures.len())
    }
}

#[derive(Clone, Copy)]
pub(in crate::smr) struct Executed<'a> {
    pub(in crate::smr) view: ExecutedView<'a>,
    pub(in crate::smr) canonical: &'a CanonicalOrder,
}

impl<'a> Executed<'a> {
    pub(in crate::smr) fn position_from(&self, entry: &Entry, from: u64) -> Option<u32> {
        let found = self.released_position_from(entry, from);
        #[cfg(debug_assertions)]
        assert_eq!(
            found,
            self.unreleased_position_from(entry, from),
            "{entry:?} from {from}: the released canonical order answers differently from the \
             full one, so a scan reached below the release line"
        );
        found
    }

    #[cfg(debug_assertions)]
    fn unreleased_position_from(&self, entry: &Entry, from: u64) -> Option<u32> {
        let hi = self.view.offset().min(self.canonical.len());
        if from < hi
            && let Some(p) = self.canonical.unreleased(from, hi).position(|e| e == entry)
        {
            return Some((from + p as u64) as u32);
        }
        self.view.position_from(entry, from).map(|p| p as u32)
    }

    fn released_position_from(&self, entry: &Entry, from: u64) -> Option<u32> {
        let offset = self.view.offset();
        if from < offset {
            let lo = from.max(self.canonical.start());
            let hi = offset.min(self.canonical.len());
            if lo < hi
                && let Some(p) = self.canonical.slice(lo, hi).iter().position(|e| e == entry)
            {
                return Some((lo + p as u64) as u32);
            }
        }
        self.view.position_from(entry, from).map(|p| p as u32)
    }

    pub(in crate::smr) fn front_scan(&self, entry: &Entry) -> Option<u32> {
        self.position_from(entry, 0)
    }

    /// `front_scan(entry).is_some()`, scanning newest first, where a recent command sits.
    pub(in crate::smr) fn holds(&self, entry: &Entry) -> bool {
        let offset = self.view.offset();
        let retained = (offset..self.view.logical_len())
            .rev()
            .any(|i| self.view.get(i) == Some(entry));
        let (lo, hi) = (self.canonical.start(), offset.min(self.canonical.len()));
        let held = retained || (lo < hi && self.canonical.slice(lo, hi).contains(entry));
        #[cfg(debug_assertions)]
        assert_eq!(
            held,
            self.unreleased_position_from(entry, 0).is_some(),
            "{entry:?}: the released canonical order answers differently from the full one"
        );
        held
    }
}

#[cfg(test)]
mod tests;
