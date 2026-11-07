//! Double-execution oracle: a (client, sn) executed twice on one server.
//!
//! The no-fork check behind `safety_ok` (every executed sequence a prefix of one order) cannot
//! see a slot every server repeats, so this latch is kept apart from it. Only `Entry::Cmd` counts: S_i holds nothing else, since ⊥ only sets
//! sn(c) and x_d is a no-op, so an executed command and a ⊥ for its slot are one execution.
//! A different op in an already executed slot does count.

use protocol::compact::{ClientId, Entry};
use protocol::shared_state::ExecutedView;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::{BTreeMap, VecDeque};

type Slot = (ClientId, u64);

pub(in crate::smr) struct ExecDupOracle {
    canon: CanonSlots,
    canon_covered: u64,
    /// Per-server slots, built only once the prefix latch is gone.
    nodes: Vec<Option<NodeSlots>>,
    tripped: Option<usize>,
}

/// Positions below `base` are the canonical order's; `slots` holds those in `[base, len)`.
struct NodeSlots {
    base: u64,
    len: u64,
    slots: FxHashSet<Slot>,
}

/// The canonical order's slots: membership of every slot it ever held, since an old slot can be
/// re-executed arbitrarily late, and first positions only at or above the release line.
#[derive(Default)]
struct CanonSlots {
    members: SlotSet,
    /// A member without an entry here first sat below `released`.
    first: FxHashMap<Slot, u64>,
    order: VecDeque<(u64, Slot)>,
    released: u64,
    #[cfg(debug_assertions)]
    shadow: FxHashMap<Slot, u64>,
}

impl CanonSlots {
    /// False when `s` was already a member, i.e. the order repeats it at `at`.
    fn insert(&mut self, s: Slot, at: u64) -> bool {
        let fresh = self.members.insert(s);
        #[cfg(debug_assertions)]
        {
            let shadow_fresh = !self.shadow.contains_key(&s);
            if shadow_fresh {
                self.shadow.insert(s, at);
            }
            assert_eq!(
                fresh, shadow_fresh,
                "slot {s:?}: membership disagrees with the full index"
            );
        }
        if fresh {
            self.first.insert(s, at);
            self.order.push_back((at, s));
        }
        fresh
    }

    fn release_to(&mut self, upto: u64) {
        if upto <= self.released {
            return;
        }
        self.released = upto;
        while let Some(&(at, s)) = self.order.front()
            && at < upto
        {
            self.order.pop_front();
            self.first.remove(&s);
        }
        // Returned only once the window shrank well below a burst's capacity.
        if self.first.len() * 4 < self.first.capacity() {
            self.first.shrink_to_fit();
            self.order.shrink_to_fit();
        }
    }

    /// Whether `s` first entered the order below `base`.
    fn first_below(&self, s: Slot, base: u64) -> bool {
        assert!(
            base >= self.released,
            "a server's executed base {base} is below the oracle's release line {} — the \
             positions below it were released, so the double-execution verdict would be a guess",
            self.released
        );
        let below = match self.first.get(&s) {
            Some(&at) => at < base,
            None => self.members.contains(s),
        };
        #[cfg(debug_assertions)]
        assert_eq!(
            below,
            self.shadow.get(&s).is_some_and(|&at| at < base),
            "slot {s:?}: the released index disagrees with the full one at base {base}"
        );
        below
    }

    fn heap_bytes(&self) -> u64 {
        self.members.heap_bytes()
            + (self.first.capacity() * (size_of::<(Slot, u64)>() + 1)
                + self.order.capacity() * size_of::<(u64, Slot)>()) as u64
    }
}

/// Per client, every sn in `1..=floor` plus the sparse members above it. A floor of exactly 1,
/// a single-command client's whole history, is kept in runs of client ids.
#[derive(Default)]
struct SlotSet {
    /// First client of a run -> its last client, inclusive.
    ones: BTreeMap<ClientId, ClientId>,
    floor: FxHashMap<ClientId, u64>,
    above: FxHashSet<Slot>,
}

impl SlotSet {
    fn floor_of(&self, client: ClientId) -> u64 {
        if let Some(&floor) = self.floor.get(&client) {
            return floor;
        }
        let in_run = self
            .ones
            .range(..=client)
            .next_back()
            .is_some_and(|(_, &last)| client <= last);
        u64::from(in_run)
    }

    fn contains(&self, (client, sn): Slot) -> bool {
        (sn >= 1 && sn <= self.floor_of(client)) || self.above.contains(&(client, sn))
    }

    /// False when `slot` was already a member.
    fn insert(&mut self, slot: Slot) -> bool {
        if self.contains(slot) {
            return false;
        }
        let (client, sn) = slot;
        let floor = self.floor_of(client);
        if sn != floor + 1 {
            self.above.insert(slot);
            return true;
        }
        let mut raised = sn;
        while self.above.remove(&(client, raised + 1)) {
            raised += 1;
        }
        if raised == 1 {
            self.join_run(client);
        } else {
            if floor == 1 {
                self.leave_run(client);
            }
            self.floor.insert(client, raised);
        }
        true
    }

    fn join_run(&mut self, client: ClientId) {
        let mut first = client;
        if let Some(before) = client.checked_sub(1)
            && let Some((&lo, &hi)) = self.ones.range(..=before).next_back()
            && hi == before
        {
            first = lo;
        }
        let last = client
            .checked_add(1)
            .and_then(|after| self.ones.remove(&after))
            .unwrap_or(client);
        self.ones.insert(first, last);
    }

    fn leave_run(&mut self, client: ClientId) {
        let (&first, &last) = self
            .ones
            .range(..=client)
            .next_back()
            .filter(|&(_, &last)| client <= last)
            .expect("a floor of 1 lies in a run");
        self.ones.remove(&first);
        if first < client {
            self.ones.insert(first, client - 1);
        }
        if client < last {
            self.ones.insert(client + 1, last);
        }
    }

    fn heap_bytes(&self) -> u64 {
        ((self.ones.len() * size_of::<(ClientId, ClientId)>() * 3) / 2
            + self.floor.capacity() * (size_of::<(ClientId, u64)>() + 1)
            + self.above.capacity() * (size_of::<Slot>() + 1)) as u64
    }
}

fn slot(e: &Entry) -> Option<Slot> {
    match e {
        Entry::Cmd(c) => Some((c.client, c.sn)),
        Entry::Null { .. } | Entry::Nop(_) => None,
    }
}

impl ExecDupOracle {
    pub(in crate::smr) fn new(n: usize) -> Self {
        ExecDupOracle {
            canon: CanonSlots::default(),
            canon_covered: 0,
            nodes: (0..n).map(|_| None).collect(),
            tripped: None,
        }
    }

    pub(in crate::smr) fn tripped_round(&self) -> Option<usize> {
        self.tripped
    }

    pub(in crate::smr) fn heap_bytes(&self) -> u64 {
        let nodes: usize = self
            .nodes
            .iter()
            .flatten()
            .map(|n| n.slots.capacity() * (size_of::<Slot>() + 1))
            .sum();
        self.canon.heap_bytes() + nodes as u64
    }

    /// First positions still held.
    #[cfg(test)]
    pub(in crate::smr) fn resident_positions(&self) -> usize {
        self.canon.first.len()
    }

    fn trip(&mut self, round: usize) {
        self.tripped = Some(round);
        self.canon = CanonSlots::default();
        self.nodes.iter_mut().for_each(|n| *n = None);
    }

    /// While every executed sequence is a prefix of the canonical order, a server repeats a slot
    /// exactly when the canonical order does, in the same round. `start`, the order's release
    /// line, is where the first positions this keeps begin.
    pub(in crate::smr) fn observe_canonical(
        &mut self,
        start: u64,
        retained: &[Entry],
        round: usize,
    ) {
        if self.tripped.is_some() {
            return;
        }
        assert!(
            start <= self.canon_covered,
            "the canonical order starts at {start}, past the {} positions the oracle saw",
            self.canon_covered
        );
        let skip = (self.canon_covered - start) as usize;
        for (i, e) in retained.iter().enumerate().skip(skip) {
            let at = start + i as u64;
            if let Some(s) = slot(e)
                && !self.canon.insert(s, at)
            {
                self.trip(round);
                return;
            }
        }
        self.canon_covered = self.canon_covered.max(start + retained.len() as u64);
        self.canon.release_to(start);
    }

    /// After the prefix latch: `replaced[i]` marks a server whose S_i may have been swapped
    /// this step (compact: pre-step L_i = ⊥; recovery: adoption or rollback); every other
    /// server only appended.
    pub(in crate::smr) fn observe_nodes(
        &mut self,
        executed: &[ExecutedView<'_>],
        replaced: &[bool],
        round: usize,
    ) {
        if self.tripped.is_some() {
            return;
        }
        let canon_covered = self.canon_covered;
        let canon = &self.canon;
        for (i, view) in executed.iter().enumerate() {
            let node = match &mut self.nodes[i] {
                Some(node) if !replaced[i] => {
                    assert!(
                        view.offset() == node.base && view.logical_len() >= node.len,
                        "round {round}: server {i} changed its executed prefix without a ⊥ log"
                    );
                    node
                }
                vacant => vacant.insert(NodeSlots {
                    base: view.offset(),
                    len: view.offset(),
                    slots: FxHashSet::default(),
                }),
            };
            assert!(
                node.base <= canon_covered,
                "round {round}: server {i} forgot past the canonical order the oracle saw"
            );
            let base = node.base;
            let repeated = view
                .iter_from(node.len)
                .filter_map(slot)
                .any(|s| canon.first_below(s, base) || !node.slots.insert(s));
            if repeated {
                self.trip(round);
                return;
            }
            node.len = view.logical_len();
        }
    }
}

#[cfg(test)]
mod tests;
