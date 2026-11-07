//! One `BlockingStrategy` per `BlockSchedule` variant. RNG contract: each impl draws exactly
//! what the pre-trait driver loops drew, in the same order (pinned by the blocking golden test).

use super::noise_table::ActionTable;
use super::view::{Knowledge, View};
use super::{BlockingStrategy, adaptive_mask, sample_blocked, sticky_with_background};
use rand::seq::index::sample;
use rand_chacha::ChaCha12Rng;

pub(super) struct Fresh {
    pub(super) n: usize,
    pub(super) count: usize,
}

impl BlockingStrategy for Fresh {
    fn knowledge(&self) -> Knowledge {
        Knowledge::OwnHistory
    }

    fn max_blocked(&self) -> usize {
        self.count
    }

    fn mask(&mut self, _view: &View<'_>, rng: &mut ChaCha12Rng) -> Vec<bool> {
        sample_blocked(rng, self.n, self.count)
    }

    fn done_blocking_after(&self, _round: usize) -> bool {
        self.count == 0
    }
}

pub(super) struct Fixed(pub(super) Vec<bool>);

impl BlockingStrategy for Fixed {
    fn knowledge(&self) -> Knowledge {
        Knowledge::OwnHistory
    }

    fn max_blocked(&self) -> usize {
        blocked_count(&self.0)
    }

    fn mask(&mut self, _view: &View<'_>, _rng: &mut ChaCha12Rng) -> Vec<bool> {
        self.0.clone()
    }

    fn done_blocking_after(&self, _round: usize) -> bool {
        !self.0.contains(&true)
    }
}

/// Inclusive 1-based (start, end) rounds per window, each with its compiled mask.
pub(super) struct Windows {
    pub(super) n: usize,
    pub(super) windows: Vec<(usize, usize, Vec<bool>)>,
}

fn blocked_count(mask: &[bool]) -> usize {
    mask.iter().filter(|&&b| b).count()
}

/// A union only grows at a window start, so the maximum over rounds is attained at one.
fn union_at(windows: &[(usize, usize, Vec<bool>)], n: usize, round: usize) -> Vec<bool> {
    let mut union = vec![false; n];
    for (start, end, mask) in windows {
        if (*start..=*end).contains(&round) {
            for (u, &m) in union.iter_mut().zip(mask) {
                *u |= m;
            }
        }
    }
    union
}

impl BlockingStrategy for Windows {
    fn knowledge(&self) -> Knowledge {
        Knowledge::OwnHistory
    }

    fn max_blocked(&self) -> usize {
        self.windows
            .iter()
            .map(|(start, _, _)| blocked_count(&union_at(&self.windows, self.n, *start)))
            .max()
            .unwrap_or(0)
    }

    fn mask(&mut self, view: &View<'_>, _rng: &mut ChaCha12Rng) -> Vec<bool> {
        union_at(&self.windows, self.n, view.round())
    }

    fn done_blocking_after(&self, round: usize) -> bool {
        self.windows
            .iter()
            .all(|(_, end, mask)| *end <= round || !mask.contains(&true))
    }
}

pub(super) struct PerRound {
    pub(super) n: usize,
    pub(super) counts: Vec<usize>,
}

impl BlockingStrategy for PerRound {
    fn knowledge(&self) -> Knowledge {
        Knowledge::OwnHistory
    }

    fn max_blocked(&self) -> usize {
        self.counts.iter().copied().max().unwrap_or(0)
    }

    fn mask(&mut self, view: &View<'_>, rng: &mut ChaCha12Rng) -> Vec<bool> {
        let count = self.counts.get(view.round() - 1).copied().unwrap_or(0);
        sample_blocked(rng, self.n, count)
    }

    fn done_blocking_after(&self, round: usize) -> bool {
        self.counts.iter().skip(round).all(|&c| c == 0)
    }
}

pub(super) struct Sticky {
    pub(super) n: usize,
    pub(super) background_count: usize,
    pub(super) counts: Vec<usize>,
    pub(super) sticky: Vec<bool>,
}

impl BlockingStrategy for Sticky {
    fn knowledge(&self) -> Knowledge {
        Knowledge::OwnHistory
    }

    fn max_blocked(&self) -> usize {
        (self.counts.iter().copied().max().unwrap_or(0) + self.background_count).min(self.n)
    }

    fn mask(&mut self, view: &View<'_>, rng: &mut ChaCha12Rng) -> Vec<bool> {
        let count = self.counts.get(view.round() - 1).copied().unwrap_or(0);
        sticky_with_background(rng, &mut self.sticky, count, self.background_count, self.n)
    }

    fn done_blocking_after(&self, round: usize) -> bool {
        self.background_count == 0 && self.counts.iter().skip(round).all(|&c| c == 0)
    }
}

/// `BlockHolders` policy: block holders from the delayed snapshot, uniform before one exists.
pub(super) struct BlockHolders {
    pub(super) n: usize,
    pub(super) count: usize,
    pub(super) alpha: u32,
}

impl BlockingStrategy for BlockHolders {
    fn knowledge(&self) -> Knowledge {
        Knowledge::NodeState { alpha: self.alpha }
    }

    fn max_blocked(&self) -> usize {
        self.count
    }

    fn mask(&mut self, view: &View<'_>, rng: &mut ChaCha12Rng) -> Vec<bool> {
        match view.holders() {
            Some(holders) => adaptive_mask(rng, holders, self.n, self.count),
            None => sample_blocked(rng, self.n, self.count),
        }
    }

    fn done_blocking_after(&self, _round: usize) -> bool {
        self.count == 0
    }
}

/// Exploratory worst-case 1-late availability adversary: the full budget, all of it fresh.
pub(super) struct Disjoint {
    pub(super) n: usize,
    pub(super) count: usize,
    pub(super) previous: Vec<bool>,
}

impl BlockingStrategy for Disjoint {
    fn knowledge(&self) -> Knowledge {
        Knowledge::OwnHistory
    }

    fn max_blocked(&self) -> usize {
        self.count
    }

    fn mask(&mut self, _view: &View<'_>, rng: &mut ChaCha12Rng) -> Vec<bool> {
        let fresh: Vec<usize> = (0..self.n).filter(|&i| !self.previous[i]).collect();
        assert!(
            self.count <= fresh.len(),
            "disjoint: {} fresh servers for a budget of {}",
            fresh.len(),
            self.count
        );
        let mut mask = vec![false; self.n];
        for pick in sample(rng, fresh.len(), self.count) {
            mask[fresh[pick]] = true;
        }
        self.previous.clone_from(&mask);
        mask
    }

    fn done_blocking_after(&self, _round: usize) -> bool {
        self.count == 0
    }
}

/// Exploratory worst-case 1-late adversary against two-value agreement, played from a DP table.
pub(super) struct NoiseController {
    pub(super) n: usize,
    pub(super) table: &'static ActionTable,
    pub(super) previous: Vec<bool>,
}

impl NoiseController {
    /// (|U|, servers of U holding the larger value); the DP is symmetric in the two values.
    fn two_value_state(counts: &[(crate::Value, usize)]) -> (usize, usize) {
        match counts {
            [] => (0, 0),
            [(_, only)] => (*only, *only),
            [(_, smaller), (_, larger)] => (smaller + larger, *larger),
            _ => panic!(
                "noise_controller is two-value only, but U_{{t-1}} holds {} values",
                counts.len()
            ),
        }
    }
}

impl BlockingStrategy for NoiseController {
    fn knowledge(&self) -> Knowledge {
        Knowledge::NodeState { alpha: 1 }
    }

    fn max_blocked(&self) -> usize {
        self.table.b()
    }

    fn mask(&mut self, view: &View<'_>, rng: &mut ChaCha12Rng) -> Vec<bool> {
        let (u, a1) = match view.useful_value_counts() {
            Some(counts) => Self::two_value_state(counts),
            None => {
                assert_eq!(
                    view.round(),
                    1,
                    "a 1-late view lacks U_{{t-1}} only in round 1"
                );
                (self.n, self.n / 2)
            }
        };
        let fresh: Vec<usize> = (0..self.n).filter(|&i| !self.previous[i]).collect();
        let stale: Vec<usize> = (0..self.n).filter(|&i| self.previous[i]).collect();
        let (m, o) = self.table.action(stale.len(), u, a1);
        let mut mask = vec![false; self.n];
        for (pool, count) in [(&fresh, m), (&stale, o)] {
            if count > 0 {
                for pick in sample(rng, pool.len(), count) {
                    mask[pool[pick]] = true;
                }
            }
        }
        self.previous.clone_from(&mask);
        mask
    }

    fn done_blocking_after(&self, _round: usize) -> bool {
        self.table.b() == 0
    }
}
