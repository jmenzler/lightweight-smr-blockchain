//! Blocking strategies and per-round mask resolution; all mask entropy comes from the caller's RNG.
//!
//! `BlockSchedule` stays the plain-data wire surface; `compile_blocking` turns it into one
//! `BlockingStrategy`. A new adversary is one `BlockSchedule` variant plus one impl in
//! `strategies.rs`. Strategies only ever see a `View` the engine built with the
//! Definition 1.1 delay applied (`view.rs`).

pub(crate) mod noise_table;
mod strategies;
pub(crate) mod view;

use crate::{BlockSchedule, BlockTarget, BlockWindow, Config, Init};
use rand::seq::index::sample;
use rand_chacha::ChaCha12Rng;
use strategies::{
    BlockHolders, Disjoint, Fixed, Fresh, NoiseController, PerRound, Sticky, Windows,
};
pub(crate) use view::{Knowledge, SnapshotDelay, View, history_only_view};

pub(crate) trait BlockingStrategy {
    /// What the engine must put into the view; drivers without a node-state hook refuse `NodeState`.
    fn knowledge(&self) -> Knowledge;

    /// Most servers any one mask may block; the engine asserts every mask against it.
    fn max_blocked(&self) -> usize;

    /// The mask for `view.round()`; called once per round, in round order. `rng` is the
    /// stream this round's target draws continue on: draw from it, never clone or inspect it.
    fn mask(&mut self, view: &View<'_>, rng: &mut ChaCha12Rng) -> Vec<bool>;

    /// True when no later round can block anyone, which lets an agreed run stop early.
    fn done_blocking_after(&self, round: usize) -> bool;
}

/// The only way drivers obtain a mask; a hard assert, so release sweeps cannot over-block silently.
pub(crate) fn checked_mask(
    strategy: &mut dyn BlockingStrategy,
    view: &View<'_>,
    rng: &mut ChaCha12Rng,
    n: usize,
) -> Vec<bool> {
    let mask = strategy.mask(view, rng);
    assert_eq!(
        mask.len(),
        n,
        "round {}: strategy returned a mask of length {} for n = {n}",
        view.round(),
        mask.len()
    );
    let blocked = mask.iter().filter(|&&b| b).count();
    let budget = strategy.max_blocked();
    assert!(
        blocked <= budget,
        "round {}: strategy blocked {blocked} of {n} servers, over its declared budget of {budget}",
        view.round()
    );
    mask
}

pub(crate) fn fraction_in_range(name: &str, fraction: f64) -> Result<(), String> {
    if (0.0..=1.0).contains(&fraction) {
        Ok(())
    } else {
        Err(format!("{name} must be within [0, 1], got {fraction}"))
    }
}

impl BlockSchedule {
    /// The rules a spec enforces on parse, for schedules built directly in Rust.
    pub fn validate(&self, n: usize) -> Result<(), String> {
        match self {
            BlockSchedule::FreshPerRound { fraction } => {
                fraction_in_range("fresh_per_round fraction", *fraction)
            }
            BlockSchedule::Permanent { fraction } => {
                fraction_in_range("permanent fraction", *fraction)
            }
            BlockSchedule::Adaptive0Late { fraction, .. } => {
                fraction_in_range("adaptive_0late fraction", *fraction)
            }
            BlockSchedule::AdaptiveAlphaLate {
                fraction, alpha, ..
            } => {
                if *alpha == 0 {
                    return Err(
                        "adaptive_alpha_late alpha must be >= 1 (alpha = 0 is the adaptive_0late kind)"
                            .into(),
                    );
                }
                fraction_in_range("adaptive_alpha_late fraction", *fraction)
            }
            BlockSchedule::PerRoundFractions(fractions) => fractions
                .iter()
                .try_for_each(|f| fraction_in_range("per_round fraction", *f)),
            BlockSchedule::PerRoundSticky {
                background,
                targets,
            } => {
                fraction_in_range("sticky background fraction", *background)?;
                targets
                    .iter()
                    .try_for_each(|f| fraction_in_range("per_round sticky fraction", *f))
            }
            BlockSchedule::Windows(windows) => windows.iter().try_for_each(|w| w.validate(n)),
            BlockSchedule::Disjoint { fraction } => {
                fraction_in_range("disjoint fraction", *fraction)?;
                check_schedule(self, n)
            }
            BlockSchedule::NoiseController { fraction } => {
                fraction_in_range("noise_controller fraction", *fraction)?;
                check_schedule(self, n)
            }
        }
    }
}

impl BlockWindow {
    fn validate(&self, n: usize) -> Result<(), String> {
        if self.start_round < 1 {
            return Err("window start_round must be at least 1".into());
        }
        if self.rounds < 1 {
            return Err("window rounds must be at least 1".into());
        }
        if self.start_round.checked_add(self.rounds - 1).is_none() {
            return Err("window end round overflows".into());
        }
        match &self.target {
            BlockTarget::Nodes(ids) => match ids.iter().find(|&&id| id as usize >= n) {
                Some(bad) => Err(format!("window node id {bad} out of range for n={n}")),
                None => Ok(()),
            },
            BlockTarget::SampleFraction(fraction) => {
                fraction_in_range("window sample fraction", *fraction)
            }
        }
    }
}

// RNG contract: masks are sampled up front in declaration order.
pub(crate) fn compile_blocking(
    schedule: &BlockSchedule,
    n: usize,
    rng: &mut ChaCha12Rng,
) -> Box<dyn BlockingStrategy> {
    match schedule {
        BlockSchedule::FreshPerRound { fraction } => Box::new(Fresh {
            n,
            count: fraction_count(*fraction, n),
        }),
        BlockSchedule::Adaptive0Late { fraction, .. } => Box::new(BlockHolders {
            n,
            count: fraction_count(*fraction, n),
            alpha: 0,
        }),
        BlockSchedule::AdaptiveAlphaLate {
            fraction, alpha, ..
        } => Box::new(BlockHolders {
            n,
            count: fraction_count(*fraction, n),
            alpha: *alpha,
        }),
        BlockSchedule::Permanent { fraction } => {
            let count = fraction_count(*fraction, n);
            Box::new(Fixed(sample_blocked(rng, n, count)))
        }
        BlockSchedule::Windows(windows) => Box::new(Windows {
            n,
            windows: windows
                .iter()
                .map(|w| {
                    let mask = match &w.target {
                        BlockTarget::Nodes(ids) => {
                            let mut mask = vec![false; n];
                            for &id in ids {
                                mask[id as usize] = true;
                            }
                            mask
                        }
                        BlockTarget::SampleFraction(fraction) => {
                            sample_blocked(rng, n, fraction_count(*fraction, n))
                        }
                    };
                    (w.start_round, w.start_round + w.rounds - 1, mask)
                })
                .collect(),
        }),
        BlockSchedule::PerRoundFractions(fractions) => Box::new(PerRound {
            n,
            counts: fractions.iter().map(|f| fraction_count(*f, n)).collect(),
        }),
        BlockSchedule::PerRoundSticky {
            background,
            targets,
        } => Box::new(Sticky {
            n,
            background_count: fraction_count(*background, n),
            counts: targets.iter().map(|f| fraction_count(*f, n)).collect(),
            sticky: vec![false; n],
        }),
        BlockSchedule::Disjoint { fraction } => {
            disjoint_precondition(*fraction).unwrap_or_else(|e| panic!("{e}"));
            Box::new(Disjoint {
                n,
                count: fraction_count(*fraction, n),
                previous: vec![false; n],
            })
        }
        BlockSchedule::NoiseController { fraction } => {
            let count = fraction_count(*fraction, n);
            let table = noise_table::lookup(n, count)
                .unwrap_or_else(|| panic!("{}", noise_table::missing_table(n, count)));
            Box::new(NoiseController {
                n,
                table,
                previous: vec![false; n],
            })
        }
    }
}

/// Proof v5's precondition β ≤ 1/2: then n − ⌊βn⌋ ≥ ⌊βn⌋ fresh servers always exist.
pub(crate) fn disjoint_precondition(fraction: f64) -> Result<(), String> {
    if fraction <= 0.5 {
        Ok(())
    } else {
        Err(format!(
            "disjoint needs fraction <= 1/2 so that enough servers stay unblocked from the \
             previous round, got {fraction}"
        ))
    }
}

/// Schedule preconditions beyond the generic [0, 1] fraction check.
pub(crate) fn check_schedule(schedule: &BlockSchedule, n: usize) -> Result<(), String> {
    match schedule {
        BlockSchedule::Disjoint { fraction } => disjoint_precondition(*fraction),
        BlockSchedule::NoiseController { fraction } => {
            let count = fraction_count(*fraction, n);
            match noise_table::lookup(n, count) {
                Some(_) => Ok(()),
                None => Err(noise_table::missing_table(n, count)),
            }
        }
        _ => Ok(()),
    }
}

/// Scenario-level preconditions: the noise-controller tables are optimal only for the model the DP solved.
pub(crate) fn check_scenario_fit(
    schedule: &BlockSchedule,
    cfg: Config,
    init: &Init,
    partition: Option<&[u32]>,
) -> Result<(), String> {
    if !matches!(schedule, BlockSchedule::NoiseController { .. }) {
        return Ok(());
    }
    if (cfg.k, cfg.ell) != (6, 3) {
        return Err(format!(
            "noise_controller tables are solved for the (6,3) rule, got ({},{})",
            cfg.k, cfg.ell
        ));
    }
    if partition.is_some() {
        return Err("noise_controller tables are solved without a partition".into());
    }
    if !matches!(init, Init::UniformRandom { k: 2 }) {
        return Err(format!(
            "noise_controller tables are solved for an iid two-value start (init \
             uniform_random k=2), got {init:?}"
        ));
    }
    Ok(())
}

/// For drivers with no node-state hook: a strategy that reads node state would silently fall back to uniform here.
pub(crate) fn compile_history_only(
    schedule: &BlockSchedule,
    n: usize,
    rng: &mut ChaCha12Rng,
) -> Box<dyn BlockingStrategy> {
    let strategy = compile_blocking(schedule, n, rng);
    assert!(
        !strategy.knowledge().reads_node_state(),
        "node-state schedules are drawn by the Alg-1 run loop only: {schedule:?}"
    );
    strategy
}

/// What the schedule's strategy reads; compiled on a throwaway RNG, so the caller's stream is untouched.
pub(crate) fn schedule_knowledge(schedule: &BlockSchedule, n: usize) -> Knowledge {
    use rand::SeedableRng;
    compile_blocking(schedule, n, &mut ChaCha12Rng::seed_from_u64(0)).knowledge()
}

/// Full per-round blocked-mask matrix (index 0 = round 1), in the driver loops' draw order.
pub fn mask_schedule(
    schedule: &BlockSchedule,
    n: usize,
    rounds: usize,
    rng: &mut ChaCha12Rng,
) -> Vec<Vec<bool>> {
    let mut strategy = compile_history_only(schedule, n, rng);
    (1..=rounds)
        .map(|round| checked_mask(strategy.as_mut(), &history_only_view(round), rng, n))
        .collect()
}

// RNG contract: the sticky adjustment draws first; background noise draws only when nonzero.
pub(crate) fn sticky_with_background(
    rng: &mut ChaCha12Rng,
    sticky: &mut [bool],
    count: usize,
    background_count: usize,
    n: usize,
) -> Vec<bool> {
    adjust_sticky_mask(rng, sticky, count, n);
    if background_count > 0 {
        let mut union = sample_blocked(rng, n, background_count);
        for (u, &s) in union.iter_mut().zip(sticky.iter()) {
            *u |= s;
        }
        union
    } else {
        sticky.to_vec()
    }
}

// RNG contract: steady and full-set transitions draw nothing.
pub(crate) fn adjust_sticky_mask(rng: &mut ChaCha12Rng, mask: &mut [bool], count: usize, n: usize) {
    let count = count.min(n);
    let blocked: Vec<usize> = (0..n).filter(|&i| mask[i]).collect();
    match count.cmp(&blocked.len()) {
        std::cmp::Ordering::Greater => {
            let free: Vec<usize> = (0..n).filter(|&i| !mask[i]).collect();
            let delta = count - blocked.len();
            if delta >= free.len() {
                for i in free {
                    mask[i] = true;
                }
            } else {
                for pick in sample(rng, free.len(), delta) {
                    mask[free[pick]] = true;
                }
            }
        }
        std::cmp::Ordering::Less => {
            let delta = blocked.len() - count;
            if delta >= blocked.len() {
                mask.fill(false);
            } else {
                for pick in sample(rng, blocked.len(), delta) {
                    mask[blocked[pick]] = false;
                }
            }
        }
        std::cmp::Ordering::Equal => {}
    }
}

pub(crate) fn fraction_count(fraction: f64, n: usize) -> usize {
    if n == 0 {
        return 0;
    }

    let mut count = ((fraction * n as f64).floor() as usize).min(n);
    // Multiplication and division can round to opposite sides of a boundary.
    while count > 0 && count as f64 / n as f64 > fraction {
        count -= 1;
    }
    while count < n && (count + 1) as f64 / n as f64 <= fraction {
        count += 1;
    }
    count
}

/// Smallest count c with c/n >= fraction, under the same float correction as `fraction_count`.
pub(crate) fn ceil_fraction_count(fraction: f64, n: usize) -> usize {
    let floor = fraction_count(fraction, n);
    if floor < n && (floor as f64 / n as f64) < fraction {
        floor + 1
    } else {
        floor
    }
}

// One sample: a budget covering the holders blocks them all without a draw and samples the spill from the rest; a smaller budget is sampled from the holders.
pub(crate) fn adaptive_mask(
    rng: &mut ChaCha12Rng,
    holders: &[usize],
    n: usize,
    count: usize,
) -> Vec<bool> {
    let mut blocked = vec![false; n];
    if count >= holders.len() {
        for &id in holders {
            blocked[id] = true;
        }
        let rest: Vec<usize> = (0..n).filter(|i| !blocked[*i]).collect();
        let spill = (count - holders.len()).min(rest.len());
        for i in sample(rng, rest.len(), spill) {
            blocked[rest[i]] = true;
        }
    } else {
        for i in sample(rng, holders.len(), count) {
            blocked[holders[i]] = true;
        }
    }
    blocked
}

pub(crate) fn sample_blocked(rng: &mut ChaCha12Rng, n: usize, count: usize) -> Vec<bool> {
    let mut blocked = vec![false; n];
    for i in sample(rng, n, count.min(n)) {
        blocked[i] = true;
    }
    blocked
}

#[cfg(test)]
mod tests;
