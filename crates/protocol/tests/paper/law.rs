//! Hand-derived outcome laws for the uniform ℓ-subset rules, and the check that seeded runs
//! follow them. The expected laws are counted by hand from C(r, ℓ), never computed by the
//! sampler under test.

use protocol::Value;
use std::collections::BTreeMap;

pub const TRIALS: u64 = 4000;

/// Asserts that `outcomes` (one adopted value per trial) follow `law = [(value, probability)]`.
/// A value outside the law, or with probability 0, must never occur; every other count must lie
/// within 5 standard deviations of its binomial mean.
pub fn assert_follows_law(outcomes: &[Value], law: &[(Value, f64)]) {
    let n = outcomes.len() as f64;
    let mut counts: BTreeMap<Value, usize> = BTreeMap::new();
    for &v in outcomes {
        *counts.entry(v).or_default() += 1;
    }
    for (&v, &c) in &counts {
        assert!(
            law.iter().any(|&(w, p)| w == v && p > 0.0),
            "value {v} occurred {c}x but has probability 0 under the law"
        );
    }
    for &(v, p) in law.iter().filter(|&&(_, p)| p > 0.0) {
        let observed = counts.get(&v).copied().unwrap_or(0) as f64 / n;
        let tolerance = 5.0 * (p * (1.0 - p) / n).sqrt();
        assert!(
            (observed - p).abs() <= tolerance,
            "value {v}: observed frequency {observed:.4}, law {p:.4} (tolerance {tolerance:.4})"
        );
    }
}

/// Asserts that an event occurred in `hits` of `trials` independent seeded trials with
/// probability `p`, within 5 binomial standard deviations.
pub fn assert_frequency(what: &str, hits: u64, trials: u64, p: f64) {
    let n = trials as f64;
    let observed = hits as f64 / n;
    let tolerance = 5.0 * (p * (1.0 - p) / n).sqrt();
    assert!(
        (observed - p).abs() <= tolerance,
        "{what}: observed frequency {observed:.4}, expected {p:.4} (tolerance {tolerance:.4})"
    );
}
