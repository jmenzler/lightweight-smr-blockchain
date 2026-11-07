//! The median-rule merge shared by Alg 5 and Alg 6; it draws nothing and writes through to no node.

use crate::chunked::ChunkSeq;
use crate::compact::{ClientCommand, Entry, Timed};
use crate::log::{Log, MergedLog};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Chosen<'a> {
    pub log: &'a ChunkSeq<Timed>,
    pub perm: &'a [u32],
    /// Position in the caller's draw order; survives the in-place median sort (pointer-matching is unsound).
    pub origin: u32,
}

/// Whether the median comparator reads the attached round stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StampTie {
    #[default]
    IncludeRound,
    CommandsOnly,
}

/// Which stamp survives when the union walk sees the same command again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnionStamp {
    #[default]
    FirstSighting,
    LastSighting,
}

enum Probe {
    Fresh,
    Dup(u32),
}

/// `median_len` is |L'_i|: positions below it are the median's and keep their stamps (Alg 5 takes L'_i whole).
fn apply_union_probe(
    merged: &mut ChunkSeq<Timed>,
    index: &mut PermIndex,
    entry: &Entry,
    round: u64,
    union: UnionStamp,
    median_len: usize,
) {
    let probe = index.probe(entry, merged.len() as u32);
    debug_assert_eq!(
        matches!(probe, Probe::Fresh),
        !merged.iter().any(|have| have.entry == *entry)
    );
    match probe {
        Probe::Fresh => merged.push(Timed {
            entry: entry.clone(),
            round,
        }),
        Probe::Dup(place) if union == UnionStamp::LastSighting && place as usize >= median_len => {
            merged.make_mut_at(place as usize).round = round;
        }
        Probe::Dup(_) => {}
    }
}

static SEED_ENTRIES: AtomicU64 = AtomicU64::new(0);
static GATHER_ENTRIES: AtomicU64 = AtomicU64::new(0);
static PROBE_ENTRIES: AtomicU64 = AtomicU64::new(0);
static FOLD_ENTRIES: AtomicU64 = AtomicU64::new(0);
static MERGES: AtomicU64 = AtomicU64::new(0);
static CHOSEN_ENTRIES: AtomicU64 = AtomicU64::new(0);
static CP_ENTRIES: AtomicU64 = AtomicU64::new(0);
static ALLOC_ENTRIES: AtomicU64 = AtomicU64::new(0);
static SKIPPED_ENTRIES: AtomicU64 = AtomicU64::new(0);

fn bump(c: &AtomicU64, by: usize) {
    c.fetch_add(by as u64, Ordering::Relaxed);
}

/// `(median-seed copies, from_carried gathers, union probes, fold moves)` in entries since process start.
pub fn merge_touch_counts() -> (u64, u64, u64, u64) {
    (
        SEED_ENTRIES.load(Ordering::Relaxed),
        GATHER_ENTRIES.load(Ordering::Relaxed),
        PROBE_ENTRIES.load(Ordering::Relaxed),
        FOLD_ENTRIES.load(Ordering::Relaxed),
    )
}

/// `(merges, chosen-log entries, common-prefix entries, merged-log entries allocated)` since process start.
pub fn merge_shape_counts() -> (u64, u64, u64, u64) {
    (
        MERGES.load(Ordering::Relaxed),
        CHOSEN_ENTRIES.load(Ordering::Relaxed),
        CP_ENTRIES.load(Ordering::Relaxed),
        ALLOC_ENTRIES.load(Ordering::Relaxed),
    )
}

pub fn seed_with_median(merged: &mut ChunkSeq<Timed>, median: &ChunkSeq<Timed>) {
    bump(&SEED_ENTRIES, median.len());
    merged.adopt(median);
}

/// Counts a merged log of `len` entries as allocated, once, when `Log::sealed` freezes it.
pub(crate) fn count_sealed(len: usize) {
    bump(&ALLOC_ENTRIES, len);
}

/// Common-prefix length of the chosen logs under `Timed` equality (entry and stamp).
pub fn common_prefix_len(chosen: &[Chosen<'_>]) -> usize {
    let (first, rest) = chosen.split_first().expect("at least one chosen log");
    let shortest = rest.iter().fold(first.log.len(), |m, l| m.min(l.log.len()));
    let shared = rest
        .iter()
        .map(|l| first.log.shared_prefix_chunks(l.log))
        .min()
        .unwrap_or(0);
    let floor = first.log.live_len_of_sealed_prefix(shared).min(shortest);
    let cp = floor
        + (floor..shortest)
            .take_while(|&i| rest.iter().all(|l| l.log.at(i) == first.log.at(i)))
            .count();
    bump(&CHOSEN_ENTRIES, chosen.iter().map(|c| c.log.len()).sum());
    bump(&CP_ENTRIES, cp);
    cp
}

/// Sorts the chosen logs by content in place and returns the median's index; all logs agree below `from`.
pub fn order_and_pick_median(
    chosen: &mut [Chosen<'_>],
    ell: usize,
    from: usize,
    tie: StampTie,
) -> usize {
    debug_assert_eq!(chosen.len(), ell, "the median is picked over ell logs");
    match tie {
        StampTie::IncludeRound => {
            chosen.sort_unstable_by(|a, b| compare_from(a.log, b.log, from, tie));
        }
        StampTie::CommandsOnly => {
            // Stable: exact command ties keep draw order.
            chosen.sort_by(|a, b| compare_from(a.log, b.log, from, tie));
        }
    }
    ell / 2
}

fn compare_from(
    a: &ChunkSeq<Timed>,
    b: &ChunkSeq<Timed>,
    from: usize,
    tie: StampTie,
) -> std::cmp::Ordering {
    let shared = a.shared_prefix_chunks(b);
    let at = a.live_len_of_sealed_prefix(shared).max(from);
    match tie {
        StampTie::IncludeRound => a.iter_at(at).cmp(b.iter_at(at)),
        StampTie::CommandsOnly => a
            .iter_at(at)
            .map(|t| &t.entry)
            .cmp(b.iter_at(at).map(|t| &t.entry)),
    }
}

/// Alg 5 step 4's `L'_i ∘ L̄` over the logs in M, given in draw order: the median by content, then the union.
pub fn median_merge<'a>(
    m: impl Iterator<Item = &'a Log>,
    ell: usize,
    appends: &[(ClientCommand, u64)],
    tie: StampTie,
    union: UnionStamp,
) -> MergedLog {
    let mut logs: Vec<Chosen<'_>> = m
        .enumerate()
        .map(|(j, log)| Chosen {
            log: log.entries(),
            perm: &log.perm()[..],
            origin: j as u32,
        })
        .collect();
    let cp = common_prefix_len(&logs);
    let at = order_and_pick_median(&mut logs, ell, cp, tie);
    let (log, index) = merge_onto_median(&logs, at, cp, appends, union);
    MergedLog::new(log, index)
}

/// The `L'_i ∘ L̄` of Alg 5 step 4, which Alg 6's Alg 3 step also runs, over timed entries: seeds the median `chosen[at]`, then unions the chosen logs and `appends` onto it. Alg 3 on bare commands is `LogNode::step`.
pub fn merge_onto_median(
    chosen: &[Chosen<'_>],
    at: usize,
    cp: usize,
    appends: &[(ClientCommand, u64)],
    union: UnionStamp,
) -> (ChunkSeq<Timed>, MergedIndex) {
    let (median_log, median_perm) = (chosen[at].log, chosen[at].perm);
    let mut log = ChunkSeq::new();
    seed_with_median(&mut log, median_log);
    debug_assert!(
        is_sorted_perm(&log, median_perm),
        "the median's carried index does not sort its own log"
    );
    let index = PermIndex::from_carried(&log, median_perm);
    debug_assert!(
        chosen.iter().all(|l| {
            l.log
                .iter()
                .take(cp)
                .all(|t| index.present().binary_search(&t.entry).is_ok())
        }),
        "a chosen log's common prefix is not contained in the median"
    );
    let index = union_into(&mut log, chosen, cp, appends, index, union);
    (log, index)
}

/// The box's union from `from` on (below it every entry is the median's own); consumes the pre-union index.
/// `merged` enters holding exactly the median L'_i.
pub fn union_into(
    merged: &mut ChunkSeq<Timed>,
    chosen: &[Chosen<'_>],
    from: usize,
    appends: &[(ClientCommand, u64)],
    mut index: PermIndex,
    union: UnionStamp,
) -> MergedIndex {
    let median_len = merged.len();
    let mut probes = 0usize;
    let mut skipped = 0usize;
    for c in chosen {
        // Chunk skip is sound only because nothing is freed during the walk (no ABA on chunk addresses).
        let shared = c.log.shared_prefix_chunks(merged);
        let at = c.log.live_len_of_sealed_prefix(shared).max(from);
        skipped += at
            .saturating_sub(from)
            .min(c.log.len().saturating_sub(from));
        for t in c.log.iter_at(at) {
            probes += 1;
            apply_union_probe(merged, &mut index, &t.entry, t.round, union, median_len);
        }
    }
    for (x, sent_round) in appends {
        probes += 1;
        let entry = Entry::Cmd(*x);
        apply_union_probe(merged, &mut index, &entry, *sent_round, union, median_len);
    }
    bump(&PROBE_ENTRIES, probes);
    bump(&SKIPPED_ENTRIES, skipped);
    MERGES.fetch_add(1, Ordering::Relaxed);
    MergedIndex {
        present: index.present,
        perm: index.perm,
        overflow: index.overflow,
    }
}

/// True iff the merge result is element-for-element the median: every log has length `from`, nothing appended.
pub fn union_is_noop(chosen: &[Chosen<'_>], from: usize, appends: &[(ClientCommand, u64)]) -> bool {
    let noop = appends.is_empty() && chosen.iter().all(|c| c.log.len() == from);
    TESTED.fetch_add(1, Ordering::Relaxed);
    if noop {
        NOOP.fetch_add(1, Ordering::Relaxed);
    }
    noop
}

static TESTED: AtomicU64 = AtomicU64::new(0);
static NOOP: AtomicU64 = AtomicU64::new(0);

/// `(fast-path hits, merges that tested for one)` since process start.
pub fn noop_union_counts() -> (u64, u64) {
    (NOOP.load(Ordering::Relaxed), TESTED.load(Ordering::Relaxed))
}

pub fn union_skipped_entries() -> u64 {
    SKIPPED_ENTRIES.load(Ordering::Relaxed)
}

/// Log positions ordered by entry; u32 because a log carries only the non-committed tail.
pub fn sorted_perm(log: &ChunkSeq<Timed>) -> Vec<u32> {
    let mut perm: Vec<u32> = (0..log.len() as u32).collect();
    perm.sort_unstable_by(|&a, &b| {
        let (x, y) = (log.at(a as usize), log.at(b as usize));
        x.expect("perm names a live position")
            .entry
            .cmp(&y.expect("perm names a live position").entry)
    });
    perm
}

pub fn is_sorted_perm(log: &ChunkSeq<Timed>, perm: &[u32]) -> bool {
    let mut places: Vec<u32> = perm.to_vec();
    places.sort_unstable();
    let mut sorted: Vec<&Entry> = log.iter().map(|t| &t.entry).collect();
    sorted.sort_unstable();
    places.iter().copied().eq(0..log.len() as u32)
        && perm
            .iter()
            .map(|&i| {
                &log.at(i as usize)
                    .expect("perm names a live position")
                    .entry
            })
            .eq(sorted)
}

pub struct PermIndex {
    present: Vec<Entry>,
    perm: Vec<u32>,
    overflow: Vec<(Entry, u32)>,
}

impl PermIndex {
    pub fn from_carried(log: &ChunkSeq<Timed>, perm: &[u32]) -> Self {
        assert_eq!(
            perm.len(),
            log.len(),
            "a carried index must name every position of its log once"
        );
        bump(&GATHER_ENTRIES, perm.len());
        let live: Vec<&Entry> = log.iter().map(|t| &t.entry).collect();
        PermIndex {
            present: perm
                .iter()
                .map(|&i| (*live.get(i as usize).expect("perm names a live position")).clone())
                .collect(),
            perm: perm.to_vec(),
            overflow: Vec::new(),
        }
    }

    pub fn present(&self) -> &[Entry] {
        debug_assert!(
            self.overflow.is_empty(),
            "a pre-union index never holds additions"
        );
        &self.present
    }

    fn probe(&mut self, entry: &Entry, place: u32) -> Probe {
        if let Ok(i) = self.present.binary_search(entry) {
            return Probe::Dup(self.perm[i]);
        }
        match self.overflow.binary_search_by(|(have, _)| have.cmp(entry)) {
            Ok(i) => Probe::Dup(self.overflow[i].1),
            Err(at) => {
                self.overflow.insert(at, (entry.clone(), place));
                Probe::Fresh
            }
        }
    }

    #[cfg(test)]
    fn insert(&mut self, entry: &Entry, place: u32) -> bool {
        matches!(self.probe(entry, place), Probe::Fresh)
    }
}

pub struct MergedIndex {
    present: Vec<Entry>,
    perm: Vec<u32>,
    // Sorted by entry: both folds walk `present` monotonically and rely on it.
    overflow: Vec<(Entry, u32)>,
}

impl MergedIndex {
    pub fn present(&mut self) -> &[Entry] {
        if !self.overflow.is_empty() {
            bump(&FOLD_ENTRIES, self.present.len() + self.overflow.len());
        }
        self.materialise();
        &self.present
    }

    pub fn into_perm(mut self) -> Vec<u32> {
        if self.overflow.is_empty() {
            return self.perm;
        }
        bump(&FOLD_ENTRIES, self.present.len() + self.overflow.len());
        #[cfg(debug_assertions)]
        let full = {
            let mut twin = MergedIndex {
                present: self.present.clone(),
                perm: self.perm.clone(),
                overflow: self.overflow.clone(),
            };
            twin.materialise();
            twin.perm
        };
        let mut perm = Vec::with_capacity(self.perm.len() + self.overflow.len());
        let mut med = 0;
        for (entry, place) in std::mem::take(&mut self.overflow) {
            while med < self.present.len() && self.present[med] < entry {
                perm.push(self.perm[med]);
                med += 1;
            }
            perm.push(place);
        }
        perm.extend_from_slice(&self.perm[med..]);
        #[cfg(debug_assertions)]
        assert_eq!(
            perm, full,
            "the perm-only fold did not reproduce the full fold's perm"
        );
        perm
    }

    fn materialise(&mut self) {
        let overflow = std::mem::take(&mut self.overflow);
        if overflow.is_empty() {
            return;
        }
        #[cfg(debug_assertions)]
        let seed = (self.present.clone(), self.perm.clone());
        let total = self.present.len() + overflow.len();
        let mut present = Vec::with_capacity(total);
        let mut perm = Vec::with_capacity(total);
        let mut med = 0;
        for (entry, place) in &overflow {
            while med < self.present.len() && self.present[med] < *entry {
                present.push(self.present[med].clone());
                perm.push(self.perm[med]);
                med += 1;
            }
            present.push(entry.clone());
            perm.push(*place);
        }
        present.extend_from_slice(&self.present[med..]);
        perm.extend_from_slice(&self.perm[med..]);
        self.present = present;
        self.perm = perm;

        #[cfg(debug_assertions)]
        {
            let (mut present, mut perm) = seed;
            for (entry, place) in &overflow {
                let at = present
                    .binary_search(entry)
                    .expect_err("a fresh probe was already in the index");
                present.insert(at, entry.clone());
                perm.insert(at, *place);
            }
            debug_assert_eq!(
                present, self.present,
                "the overflow fold did not reproduce repeated insertion on `present`"
            );
            debug_assert_eq!(
                perm, self.perm,
                "the overflow fold did not reproduce repeated insertion on `perm`"
            );
        }
    }
}

#[cfg(test)]
mod tests;
