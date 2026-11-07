//! The compact safety latch against a from-scratch prefix check of the live states.

use protocol::compact::Entry;
use sim::smr::{SmrReport, SmrScenario, SmrState};

fn scenario(spec: serde_json::Value) -> SmrScenario {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(spec).expect("spec parses");
    SmrScenario::try_from(spec).expect("valid scenario")
}

fn compact(n: usize, seed: u64, t_commit: u64, beta: f64) -> SmrScenario {
    scenario(serde_json::json!({
        "n": n, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "compact", "t_commit_rounds": t_commit},
        "injections": [], "max_rounds": 200,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": beta},
    }))
}

fn pairwise_comparable(seqs: &[Vec<Entry>]) -> bool {
    seqs.iter().enumerate().all(|(i, a)| {
        seqs[i + 1..].iter().all(|b| {
            let common = a.len().min(b.len());
            a[..common] == b[..common]
        })
    })
}

/// (report, first prefix-incomparable round, first round the latch is down)
fn run_with_reference(sc: &SmrScenario) -> (SmrReport, Option<usize>, Option<usize>) {
    let mut first_split = None;
    let mut first_latch = None;
    let report = sim::smr::run_smr_observed(sc, &mut |state: &SmrState| {
        if first_split.is_none() && !pairwise_comparable(&state.executed_seqs()) {
            first_split = Some(state.round());
        }
        if first_latch.is_none() && !state.safety_ok() {
            first_latch = Some(state.round());
        }
    });
    (report, first_split, first_latch)
}

// Alg 5, p. 26: every server is ⊥ in round 6 and the run dies; the fork must still be reported.
#[test]
fn compact_seed_133_reports_the_round_6_divergence() {
    let (report, split, latch) = run_with_reference(&compact(24, 133, 3, 0.25));
    assert_eq!(split, Some(6), "precondition: the fork appears in round 6");
    assert!(
        !report.safety_ok,
        "executed sequences are prefix-incomparable from round 6, but the run \
         reports safety_ok (terminal {:?})",
        report.terminal
    );
    assert_eq!(latch, split);
}

// Alg 5, p. 26: an adoption-then-commit fork must latch in its own round, not later.
#[test]
fn compact_seed_43_latches_in_the_divergence_round() {
    let (report, split, latch) = run_with_reference(&compact(256, 43, 4, 0.2));
    assert!(
        split.is_some(),
        "precondition: a divergence occurs (terminal {:?})",
        report.terminal
    );
    assert_eq!(
        latch, split,
        "the latch drops in round {latch:?}, the executed sequences diverge in round {split:?}"
    );
}
