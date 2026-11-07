//! What a blocking strategy may observe of node state. Only the engine builds a `View`, with
//! the Definition 1.1 delay already applied: a strategy never receives a handle to live node
//! state, so it cannot read the current round's state and silently become 0-late. The View
//! enforces lateness for state only. The `mask` RNG is the stream that draws this round's
//! targets next, so it is a trusted channel: strategies only draw from it, never clone or
//! inspect it (cloning would predict round-t sampling, which a 1-late adversary must not know).

use crate::Value;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Knowledge {
    /// Its own earlier masks (kept inside the strategy) and the round number; no node state.
    OwnHistory,
    /// Local states through the beginning of round t−α (Def 1.1, p. 4), exposed as the
    /// end-of-round-(t−1−α) holders and the value counts of U_{t−α}. `alpha: 0` is the
    /// out-of-model stress case.
    NodeState { alpha: u32 },
}

impl Knowledge {
    pub(crate) fn reads_node_state(self) -> bool {
        matches!(self, Knowledge::NodeState { .. })
    }
}

pub(crate) struct View<'a> {
    round: usize,
    holders: Option<&'a [usize]>,
    useful_counts: Option<&'a [(Value, usize)]>,
}

impl<'a> View<'a> {
    /// The 1-based round whose mask is being chosen.
    pub(crate) fn round(&self) -> usize {
        self.round
    }

    /// `None` until round t−1−α ≥ 1 exists, and always for `OwnHistory` strategies.
    pub(crate) fn holders(&self) -> Option<&'a [usize]> {
        self.holders
    }

    /// Value counts (ascending by value) of U_{t−α}: servers holding a value at the beginning of
    /// round t−α and not blocked in it. `None` before round α+1, for α = 0, and for `OwnHistory`.
    pub(crate) fn useful_value_counts(&self) -> Option<&'a [(Value, usize)]> {
        self.useful_counts
    }
}

/// The view for drivers without a node-state hook (SMR, precomputed mask matrix).
pub(crate) fn history_only_view(round: usize) -> View<'static> {
    View {
        round,
        holders: None,
        useful_counts: None,
    }
}

/// `alpha` as a history length; saturates where usize is narrower than u32.
fn alpha_window(alpha: u32) -> usize {
    usize::try_from(alpha).unwrap_or(usize::MAX)
}

/// Snapshots kept exactly `depth` deep, so the oldest is the only one a view exposes.
struct Delayed<T> {
    depth: Option<usize>,
    snapshots: VecDeque<T>,
}

impl<T> Delayed<T> {
    fn new(depth: Option<usize>) -> Delayed<T> {
        Delayed {
            depth,
            snapshots: VecDeque::new(),
        }
    }

    fn oldest_when_full(&self) -> Option<&T> {
        match self.depth {
            Some(depth) if self.snapshots.len() == depth => self.snapshots.front(),
            _ => None,
        }
    }

    fn record(&mut self, snapshot: impl FnOnce() -> T) {
        if let Some(depth) = self.depth {
            self.snapshots.push_back(snapshot());
            if self.snapshots.len() > depth {
                self.snapshots.pop_front();
            }
        }
    }
}

pub(crate) struct SnapshotDelay {
    holders: Delayed<Vec<usize>>,
    useful_counts: Delayed<Vec<(Value, usize)>>,
}

impl SnapshotDelay {
    pub(crate) fn new(knowledge: Knowledge) -> SnapshotDelay {
        let (holder_depth, useful_depth) = match knowledge {
            Knowledge::OwnHistory => (None, None),
            Knowledge::NodeState { alpha } => (
                Some(alpha_window(alpha).saturating_add(1)),
                (alpha >= 1).then_some(alpha_window(alpha)),
            ),
        };
        SnapshotDelay {
            holders: Delayed::new(holder_depth),
            useful_counts: Delayed::new(useful_depth),
        }
    }

    pub(crate) fn view(&self, round: usize) -> View<'_> {
        View {
            round,
            holders: self.holders.oldest_when_full().map(Vec::as_slice),
            useful_counts: self.useful_counts.oldest_when_full().map(Vec::as_slice),
        }
    }

    /// Call once per round after B_t is chosen and before the round runs, with U_t's value counts.
    pub(crate) fn record_useful(&mut self, counts: impl FnOnce() -> Vec<(Value, usize)>) {
        self.useful_counts.record(counts);
    }

    /// Call once after every round with that round's end-of-round holders; untracked strategies never evaluate it.
    pub(crate) fn record(&mut self, holders: impl FnOnce() -> Vec<usize>) {
        self.holders.record(holders);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rounds_seen(alpha: u32, rounds: usize) -> Vec<Option<usize>> {
        let mut delay = SnapshotDelay::new(Knowledge::NodeState { alpha });
        (1..=rounds)
            .map(|round| {
                let seen = delay.view(round).holders().map(|h| h[0]);
                delay.record(|| vec![round]);
                seen
            })
            .collect()
    }

    #[test]
    fn round_t_sees_the_end_of_round_t_minus_one_minus_alpha() {
        assert_eq!(
            rounds_seen(0, 4),
            vec![None, Some(1), Some(2), Some(3)],
            "alpha 0"
        );
        assert_eq!(
            rounds_seen(1, 5),
            vec![None, None, Some(1), Some(2), Some(3)],
            "alpha 1"
        );
        assert_eq!(
            rounds_seen(2, 5),
            vec![None, None, None, Some(1), Some(2)],
            "alpha 2"
        );
    }

    #[test]
    fn own_history_strategies_never_see_or_record_node_state() {
        let mut delay = SnapshotDelay::new(Knowledge::OwnHistory);
        for round in 1..=4 {
            let view = delay.view(round);
            assert!(view.holders().is_none());
            assert!(view.useful_value_counts().is_none());
            delay.record_useful(|| panic!("node state must not be read for OwnHistory"));
            delay.record(|| panic!("node state must not be read for OwnHistory"));
        }
    }

    /// Round r's useful set is recorded once B_r is chosen, labelled here by `r` as its value.
    fn useful_rounds_seen(alpha: u32, rounds: usize) -> Vec<Option<Value>> {
        let mut delay = SnapshotDelay::new(Knowledge::NodeState { alpha });
        (1..=rounds)
            .map(|round| {
                let seen = delay.view(round).useful_value_counts().map(|c| c[0].0);
                delay.record_useful(|| vec![(round as Value, 1)]);
                delay.record(|| vec![round]);
                seen
            })
            .collect()
    }

    #[test]
    fn round_t_sees_the_useful_set_of_round_t_minus_alpha_and_never_its_own() {
        assert_eq!(
            useful_rounds_seen(1, 4),
            vec![None, Some(1), Some(2), Some(3)],
            "alpha 1"
        );
        assert_eq!(
            useful_rounds_seen(2, 5),
            vec![None, None, Some(1), Some(2), Some(3)],
            "alpha 2"
        );
        assert_eq!(
            useful_rounds_seen(0, 4),
            vec![None; 4],
            "alpha 0 would be the current round, which is not defined before B_t is chosen"
        );
    }
}
