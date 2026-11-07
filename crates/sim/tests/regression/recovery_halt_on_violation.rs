//! Recovery twin of `smr_halt_on_violation`: the run stops at the first of the no-fork latch
//! flipping (a halt) and an executed-state shrink (a recorded failure, flag or not).

use sim::smr::{
    HaltKind, RunFailurePhase, SmrReport, SmrScenario, SmrState, SmrTerminal, run_smr,
    run_smr_grid, run_smr_observed,
};

fn scenario(spec: serde_json::Value) -> SmrScenario {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(spec).expect("spec parses");
    SmrScenario::try_from(spec).expect("valid scenario")
}

/// n = 32, T = 13: seed 5000702 forks by round 40 (`recovery_oracles`), seed 4420000 never does.
fn recovery_r4(seed: u64, halt: bool) -> SmrScenario {
    scenario(serde_json::json!({
        "n": 32, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": 13, "resend_until_acked": false,
                  "prefix_mismatch": "skip_boundary"},
        "repeated_commit": "execute",
        "injections": [], "max_rounds": 130,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
        "halt_on_violation": halt,
    }))
}

/// n = 150, T = 14, a heavy blocking burst in rounds 71-90; seed 9 shrinks an executed state.
fn burst(seed: u64, halt: bool) -> SmrScenario {
    let fractions: Vec<f64> = (1..=130)
        .map(|r| if (71..=90).contains(&r) { 0.8 } else { 0.1 })
        .collect();
    scenario(serde_json::json!({
        "n": 150, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": 14, "resend_until_acked": false,
                  "prefix_mismatch": "skip_boundary"},
        "injections": [], "max_rounds": 130,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.5, 0.5]},
        "schedule": {"kind": "per_round_fractions", "fractions": fractions},
        "halt_on_violation": halt,
    }))
}

/// The flag-off report and the first round whose step left the no-fork latch down.
fn with_latch_round(s: &SmrScenario) -> (SmrReport, Option<usize>) {
    let mut latch = None;
    let report = run_smr_observed(s, &mut |state: &SmrState| {
        if latch.is_none() && !state.safety_ok() {
            latch = Some(state.round());
        }
    });
    (report, latch)
}

/// The spill file is named after the full scenario, so the flag renames it; its content stays.
fn without_spill_name(mut report: SmrReport) -> (String, Option<String>) {
    let spill = report
        .spill_path
        .take()
        .map(|p| std::fs::read_to_string(p).expect("spill readable"));
    (serde_json::to_string(&report).unwrap(), spill)
}

#[test]
fn a_clean_recovery_run_is_byte_identical_with_the_flag_on() {
    for run in [run_smr, run_smr_grid] {
        let off = run(&recovery_r4(4420000, false));
        let on = run(&recovery_r4(4420000, true));
        assert!(off.safety_ok, "precondition: the cell never forks");
        assert_eq!(
            without_spill_name(off),
            without_spill_name(on),
            "the flag may not move a single byte of a clean run's report or spill"
        );
    }
}

#[test]
fn a_forking_recovery_run_halts_at_its_latch_round_as_a_fork() {
    let (full, latch) = with_latch_round(&recovery_r4(5000702, false));
    let latch = latch.expect("precondition: the cell forks");
    assert!(
        latch < 130,
        "precondition: the latch falls before the horizon"
    );
    assert_eq!(full.terminal, SmrTerminal::Ran { rounds: 130 });
    for run in [run_smr, run_smr_grid] {
        let halted = run(&recovery_r4(5000702, true));
        assert_eq!(
            halted.terminal,
            SmrTerminal::Halted {
                round: latch,
                kind: HaltKind::Fork
            }
        );
        assert!(!halted.safety_ok);
        assert_eq!(
            halted.metrics[..],
            full.metrics[..latch],
            "the halt cut the same run"
        );
    }
}

/// The shrink round of a burst seed whose flag-off run ends as an executed-state shrink, and the
/// round its no-fork latch flipped, if it did.
fn shrink_and_latch(seed: u64) -> (SmrReport, usize, Option<usize>) {
    let (off, latch) = with_latch_round(&burst(seed, false));
    let SmrTerminal::Failed {
        attempted_round,
        phase: RunFailurePhase::ExecutedRegression,
        ..
    } = off.terminal
    else {
        panic!("precondition: seed {seed} shrinks ({:?})", off.terminal)
    };
    (off, attempted_round, latch)
}

#[test]
fn a_recovery_shrink_under_the_flag_stays_a_recorded_failure() {
    let (off, shrink, latch) = shrink_and_latch(21);
    assert_eq!(
        latch, None,
        "precondition: seed 21 shrinks at {shrink} without a fork"
    );
    let on = run_smr(&burst(21, true));
    assert_eq!(
        on.terminal, off.terminal,
        "a shrink ends the run as the same failure"
    );
    assert_eq!(on.failure, off.failure);
}

#[test]
fn a_fork_before_a_shrink_halts_the_run_first() {
    let (_, shrink, latch) = shrink_and_latch(9);
    let latch = latch.expect("precondition: seed 9 forks");
    assert!(
        latch < shrink,
        "precondition: the fork at {latch} precedes the shrink at {shrink}"
    );
    let on = run_smr(&burst(9, true));
    assert_eq!(
        on.terminal,
        SmrTerminal::Halted {
            round: latch,
            kind: HaltKind::Fork
        }
    );
    assert!(on.failure.is_none());
}
