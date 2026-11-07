//! Absolute positions of the entries of a prefix-consistent executed sequence.

use protocol::compact::Entry;
use std::collections::{BTreeMap, VecDeque};

/// First positions, kept only at or above the release line.
pub(in crate::smr) struct EntryIndex {
    pos: BTreeMap<Entry, u32>,
    order: VecDeque<(u32, Entry)>,
    covered: u64,
    released: u64,
    #[cfg(debug_assertions)]
    shadow: BTreeMap<Entry, u32>,
}

impl EntryIndex {
    pub(in crate::smr) fn new() -> Self {
        EntryIndex {
            pos: BTreeMap::new(),
            order: VecDeque::new(),
            covered: 0,
            released: 0,
            #[cfg(debug_assertions)]
            shadow: BTreeMap::new(),
        }
    }

    fn skip(&self, from: u64) -> usize {
        self.covered.saturating_sub(from) as usize
    }

    pub(in crate::smr) fn extend_first(&mut self, from: u64, seq: &[Entry]) {
        for (i, e) in seq.iter().enumerate().skip(self.skip(from)) {
            let at = (from + i as u64) as u32;
            #[cfg(debug_assertions)]
            self.shadow.entry(e.clone()).or_insert(at);
            if let std::collections::btree_map::Entry::Vacant(slot) = self.pos.entry(e.clone()) {
                slot.insert(at);
                self.order.push_back((at, e.clone()));
            }
        }
        self.covered = self.covered.max(from + seq.len() as u64);
    }

    /// No query floor may lie below `upto` afterwards.
    pub(in crate::smr) fn release_to(&mut self, upto: u64) {
        if upto <= self.released {
            return;
        }
        self.released = upto;
        while let Some((at, _)) = self.order.front()
            && u64::from(*at) < upto
        {
            let (_, e) = self.order.pop_front().expect("front exists");
            self.pos.remove(&e);
        }
        if self.order.len() * 4 < self.order.capacity() {
            self.order.shrink_to_fit();
        }
    }

    pub(in crate::smr) fn heap_bytes(&self) -> u64 {
        ((self.pos.len() * (size_of::<Entry>() + size_of::<u32>())) as u64 * 3) / 2
            + (self.order.capacity() * size_of::<(u32, Entry)>()) as u64
    }

    #[cfg(test)]
    pub(in crate::smr) fn resident(&self) -> usize {
        self.pos.len()
    }

    /// First position of `entry`, for a caller that knows it occurs nowhere below `floor`.
    pub(in crate::smr) fn get_from(&self, entry: &Entry, floor: u64) -> Option<u32> {
        assert!(
            floor >= self.released,
            "query floor {floor} is below the index's release line {} — positions there were \
             released, so an answer would be a guess",
            self.released
        );
        let got = self.pos.get(entry).copied();
        #[cfg(debug_assertions)]
        {
            let full = self.shadow.get(entry).copied();
            assert!(
                full.is_none_or(|first| u64::from(first) >= floor),
                "{entry:?} occurs below the query floor {floor} (first at {full:?}), so the \
                 caller's no-occurrence-below contract is broken"
            );
            assert_eq!(
                got, full,
                "{entry:?}: the released index disagrees with the full one"
            );
        }
        got
    }
}

#[cfg(test)]
mod tests;
