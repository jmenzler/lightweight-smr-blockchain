//! A fleet that can never hold a log again is reported as wedged, not as a normal run.

use sim::smr::{SmrScenario, SmrTerminal, run_smr};

fn cell(seed: u64, t: u64, max_rounds: usize, prefix_mismatch: &str) -> SmrScenario {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(serde_json::json!({
        "n": 32, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": t, "resend_until_acked": false,
                  "prefix_mismatch": prefix_mismatch},
        "repeated_commit": "skip",
        "injections": [], "max_rounds": max_rounds,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
    }))
    .expect("spec parses");
    SmrScenario::try_from(spec).expect("valid scenario")
}

// Every node skips one boundary under DEV-3 and no newer checkpoint exists to adopt.
#[test]
fn a_fleet_wide_boundary_skip_is_reported_as_a_wedge() {
    let report = run_smr(&cell(185, 6, 72, "skip_boundary"));
    assert!(matches!(report.terminal, SmrTerminal::Ran { rounds: 72 }));
    let wedged = report.wedged_round.expect("the fleet wedges");
    assert_eq!(
        wedged % 6,
        0,
        "the skip that wedges the fleet runs at a boundary"
    );
    assert!(
        report.metrics[wedged - 1].nonbot_logs > 0,
        "a round's metrics row is taken before its boundary pass"
    );
    assert!(report.metrics[wedged..].iter().all(|m| m.nonbot_logs == 0));
    assert!(
        report.safety_ok,
        "a wedge is a liveness verdict, safety holds"
    );
}

#[test]
fn runs_that_keep_a_log_somewhere_are_not_halted() {
    for seed in 4_420_000..4_420_010 {
        for policy in ["skip_boundary", "abort"] {
            let report = run_smr(&cell(seed, 13, 130, policy));
            assert_eq!(report.wedged_round, None, "seed {seed} {policy}");
        }
    }
}

// Rolling back to the genesis checkpoint (P = ⊥) restores no log, so a surge that blanks
// window 0 wedges the fleet as well, without any DEV-3 skip.
#[test]
fn a_surge_through_window_zero_halts_on_the_genesis_checkpoint() {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(serde_json::json!({
        "n": 32, "seed": 1, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": 10, "resend_until_acked": false},
        "injections": [{"round": 30, "client": 1, "op": 7}], "max_rounds": 100,
        "schedule": {"kind": "windows", "windows": [
            {"start_round": 1, "rounds": 12, "target": {"kind": "sample_fraction", "fraction": 1.0}}
        ]},
    }))
    .expect("spec parses");
    let report = run_smr(&SmrScenario::try_from(spec).expect("valid scenario"));
    assert!(matches!(report.terminal, SmrTerminal::Ran { rounds: 100 }));
    let wedged = report.wedged_round.expect("the fleet wedges");
    assert!(wedged <= 12, "wedged at round {wedged}");
    assert_eq!(report.boundary_skip_events, 0);
    assert_eq!(report.commands[0].executed_round, None);
}
