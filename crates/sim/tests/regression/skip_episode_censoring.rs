//! Skip-episode summaries separate nodes with no round left to adopt in from nodes that stay stale.

use sim::smr::{SmrReport, SmrScenario, SmrState, run_smr_observed};

fn cell(seed: u64, t: u64, max_rounds: usize) -> SmrScenario {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(serde_json::json!({
        "n": 32, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": t, "resend_until_acked": false, "prefix_mismatch": "skip_boundary"},
        "repeated_commit": "skip",
        "injections": [], "max_rounds": max_rounds,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
    }))
    .expect("spec parses");
    SmrScenario::try_from(spec).expect("valid scenario")
}

/// Round each node's open stale episode started, read independently off the nodes.
fn run(seed: u64, t: u64, rounds: usize) -> (SmrReport, Vec<Option<usize>>, usize) {
    let sc = cell(seed, t, rounds);
    let mut open_since: Vec<Option<usize>> = vec![None; sc.n];
    let mut last_round = 0;
    let report = run_smr_observed(&sc, &mut |state: &SmrState| {
        last_round = state.round();
        for (i, since) in open_since.iter_mut().enumerate() {
            let stale = state.recovery_node(i).expect("recovery").stale_checkpoint();
            match (*since, stale) {
                (None, true) => *since = Some(last_round),
                (Some(_), false) => *since = None,
                _ => {}
            }
        }
    });
    (report, open_since, last_round)
}

// A6.46 + DRV.4: a node that skips at the final boundary had no round to adopt.
#[test]
fn skips_at_the_horizon_boundary_are_reported_apart_from_unrejoined_nodes() {
    let (report, open_since, last) = run(4420014, 13, 130);
    let at_horizon = open_since.iter().flatten().filter(|&&s| s == last).count();
    let exposed = open_since.iter().flatten().filter(|&&s| s < last).count();
    assert!(
        at_horizon > 0,
        "precondition: a node skips in the final round"
    );
    assert_eq!(report.unrejoined_nodes, exposed);
    assert_eq!(report.horizon_skip_nodes, at_horizon);
}

// A node stale for 18 rounds must show in the summary, not as a 2-round rejoin.
#[test]
fn the_longest_stale_spell_reflects_nodes_that_never_rejoin() {
    let (report, open_since, last) = run(226, 6, 60);
    let oldest = open_since
        .iter()
        .flatten()
        .min()
        .copied()
        .expect("precondition: nodes are stale at the end");
    assert_eq!(
        report.longest_stale_rounds.map(|r| r >= last - oldest),
        Some(true)
    );
    assert!(report.longest_stale_rounds >= report.max_rejoin_rounds);
}
