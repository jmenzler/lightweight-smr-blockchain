//! Recovery oracles against ground truth recomputed from the live node states after every round.
//!
//! Under Alg 6 commands commit between T-windows, and "the end of a T-window" is after its last
//! round but before the between-window steps (p. 33); the observer here runs after the whole
//! step, so it sees what each boundary committed in the boundary's own round.

use std::collections::{BTreeMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use protocol::compact::Entry;
use sim::BlockSchedule;
use sim::smr::{
    PrefixMismatch, Proto, RecoveryViolation, RunFailurePhase, SmrReport, SmrScenario, SmrState,
    SmrTerminal, run_smr, run_smr_grid, run_smr_lean, run_smr_observed,
};

fn scenario(spec: serde_json::Value) -> SmrScenario {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(spec).expect("spec parses");
    SmrScenario::try_from(spec).expect("valid scenario")
}

/// Every pair of executed sequences is prefix-comparable.
fn pairwise_comparable(seqs: &[Vec<Entry>]) -> bool {
    seqs.iter().enumerate().all(|(i, a)| {
        seqs[i + 1..].iter().all(|b| {
            let common = a.len().min(b.len());
            a[..common] == b[..common]
        })
    })
}

/// Some server's executed sequence holds one (client, sn) twice.
fn some_node_repeats(seqs: &[Vec<Entry>]) -> bool {
    seqs.iter().any(|seq| {
        let mut seen = HashSet::new();
        seq.iter()
            .filter_map(Entry::client_sn)
            .any(|slot| !seen.insert(slot))
    })
}

struct GroundTruth {
    first_inconsistent: Option<usize>,
    first_latch: Option<usize>,
    first_repeat: Option<usize>,
    same_w_fork_round: Option<usize>,
    last_round: usize,
}

/// (executed sequence, P's command sequence) of a checkpoint.
type CheckpointKey = (Vec<Entry>, Option<Vec<Entry>>);

/// Run the full (history-keeping) runner and recompute every oracle from the live states.
fn run_with_ground_truth(scenario: &SmrScenario) -> (SmrReport, GroundTruth) {
    let mut gt = GroundTruth {
        first_inconsistent: None,
        first_latch: None,
        first_repeat: None,
        same_w_fork_round: None,
        last_round: 0,
    };
    let mut minted: BTreeMap<u64, CheckpointKey> = BTreeMap::new();
    let n = scenario.n;
    let report = run_smr_observed(scenario, &mut |state: &SmrState| {
        let round = state.round();
        gt.last_round = round;
        let seqs = state.executed_seqs();
        if gt.first_inconsistent.is_none() && !pairwise_comparable(&seqs) {
            gt.first_inconsistent = Some(round);
        }
        if gt.first_latch.is_none() && !state.safety_ok() {
            gt.first_latch = Some(round);
        }
        if gt.first_repeat.is_none() && some_node_repeats(&seqs) {
            gt.first_repeat = Some(round);
        }
        for i in 0..n {
            let cp = state.recovery_node(i).expect("recovery node").checkpoint();
            let p_cmds =
                cp.p.as_ref()
                    .map(|p| p.iter().map(|t| t.entry.clone()).collect());
            let key = (cp.s.untruncated().to_vec(), p_cmds);
            match minted.get(&cp.w) {
                Some(prev) if *prev != key && gt.same_w_fork_round.is_none() => {
                    gt.same_w_fork_round = Some(round);
                }
                Some(_) => {}
                None => {
                    minted.insert(cp.w, key);
                }
            }
        }
    });
    (report, gt)
}

/// n = 32, T = 13, rate 4 per round, no blocking: the E9 ratio-4 cell with the stress client.
fn recovery_r4(seed: u64, max_rounds: usize, prefix_mismatch: &str, repeated: &str) -> SmrScenario {
    scenario(serde_json::json!({
        "n": 32, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": 13, "resend_until_acked": false,
                  "prefix_mismatch": prefix_mismatch},
        "repeated_commit": repeated,
        "injections": [], "max_rounds": max_rounds,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
    }))
}

/// n = 150, T = 14, a heavy blocking burst in rounds 71-90, light traffic.
fn burst_scenario(seed: u64, prefix_mismatch: PrefixMismatch) -> SmrScenario {
    let mut s = scenario(serde_json::json!({
        "n": 150, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": 14, "resend_until_acked": false},
        "injections": [], "max_rounds": 130,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.5, 0.5]},
        "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
    }));
    let Proto::Recovery {
        prefix_mismatch: policy,
        ..
    } = &mut s.proto
    else {
        unreachable!("a recovery scenario")
    };
    *policy = prefix_mismatch;
    s.schedule = BlockSchedule::PerRoundFractions(
        (1..=130)
            .map(|r| if (71..=90).contains(&r) { 0.8 } else { 0.1 })
            .collect(),
    );
    s
}

// Def 1.4 + Def 1.5, pp. 7-8: the state the last boundary of a run (H = 10T) produces is checked.
#[test]
fn final_boundary_divergence_on_seed_4420314_is_reported() {
    let (report, gt) = run_with_ground_truth(&recovery_r4(4420314, 130, "skip_boundary", "skip"));
    assert!(
        gt.first_inconsistent.is_some(),
        "precondition: the executed sequences diverge by round 130 (terminal {:?})",
        report.terminal
    );
    assert!(
        !report.safety_ok,
        "executed sequences are prefix-incomparable at round {:?} of a 130-round run, \
         but the report says safety_ok = true",
        gt.first_inconsistent
    );
}

// Def 1.4, p. 7: the verdict on the first 130 rounds does not depend on the run going on.
#[test]
fn horizon_130_and_131_agree_on_the_first_130_rounds_for_seed_4420314() {
    let at_130 = run_smr(&recovery_r4(4420314, 130, "skip_boundary", "skip"));
    let (at_131, gt) = run_with_ground_truth(&recovery_r4(4420314, 131, "skip_boundary", "skip"));
    assert!(
        gt.first_inconsistent.is_some_and(|r| r <= 130),
        "precondition: ground truth diverges by round 130: {:?}",
        gt.first_inconsistent
    );
    assert_eq!(at_130.safety_ok, at_131.safety_ok);
    assert!(!at_130.safety_ok);
}

// p. 35: "any checkpoints bearing the same window number are identical"; boundary 130 mints a fork.
#[test]
fn final_boundary_checkpoint_fork_on_seed_4420005_is_reported() {
    let (report, gt) = run_with_ground_truth(&recovery_r4(4420005, 130, "abort", "execute"));
    assert!(
        gt.same_w_fork_round.is_some(),
        "precondition: two different checkpoints share a window number (terminal {:?})",
        report.terminal
    );
    assert!(
        !report.recovery.as_ref().expect("recovery report").fork_ok,
        "two different checkpoints with the same window number exist from round {:?} \
         (run ends at {}), but the report says fork_ok = true",
        gt.same_w_fork_round,
        gt.last_round
    );
}

// Def 1.4, p. 7 / p. 33: n = 16, T = 6, Windows schedule, seed 34, horizon 60 = 10T.
#[test]
fn final_boundary_divergence_on_small_windows_seed_34_is_reported() {
    let sc = scenario(serde_json::json!({
        "n": 16, "seed": 34, "k": 6, "ell": 3, "sigma": 2.0,
        "proto": {"kind": "recovery", "t_window_rounds": 6, "resend_until_acked": false},
        "injections": [], "max_rounds": 60,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 1.0]},
        "schedule": {"kind": "windows", "windows": [
            {"start_round": 41, "rounds": 4, "target": {"kind": "sample_fraction", "fraction": 0.5}},
            {"start_round": 47, "rounds": 4, "target": {"kind": "sample_fraction", "fraction": 0.5}}
        ]},
    }));
    let (report, gt) = run_with_ground_truth(&sc);
    assert!(
        gt.first_inconsistent.is_some(),
        "precondition: the executed sequences diverge by round 60 (terminal {:?})",
        report.terminal
    );
    assert!(!report.safety_ok);
}

// p. 33 end-of-window: a divergence committed at boundary 39 latches in round 39.
#[test]
fn boundary_divergence_on_seed_5000702_latches_in_its_own_round() {
    let (report, gt) =
        run_with_ground_truth(&recovery_r4(5000702, 130, "skip_boundary", "execute"));
    assert!(
        gt.first_inconsistent.is_some(),
        "precondition: a divergence occurs (terminal {:?})",
        report.terminal
    );
    assert_eq!(gt.first_latch, gt.first_inconsistent);
}

// p. 30: the committed sequence must not fork; the lean runner sees the seed-5000702 split brain.
#[test]
fn lean_runner_reports_the_seed_5000702_split_brain() {
    let sc = recovery_r4(5000702, 130, "skip_boundary", "execute");
    let (full, gt) = run_with_ground_truth(&sc);
    assert!(
        gt.first_inconsistent.is_some_and(|r| r <= 40),
        "precondition: ground truth diverges by round 40: {:?}",
        gt.first_inconsistent
    );
    assert!(!full.safety_ok, "precondition: the full runner latches");
    assert!(!run_smr_lean(&sc).safety_ok);
}

// p. 30: the grid runner (sweep CSV source) reports both known split-brain seeds.
#[test]
fn grid_runner_reports_known_recovery_split_brains() {
    let mut missed = Vec::new();
    for seed in [5000702u64, 5001397] {
        let sc = recovery_r4(seed, 130, "skip_boundary", "execute");
        let (_, gt) = run_with_ground_truth(&sc);
        assert!(
            gt.first_inconsistent.is_some(),
            "precondition: seed {seed} diverges in the full runner"
        );
        if run_smr_grid(&sc).safety_ok {
            missed.push((seed, gt.first_inconsistent));
        }
    }
    assert!(
        missed.is_empty(),
        "(seed, divergence round) forked but run_smr_grid reports safety_ok = true: {missed:?}"
    );
}

// p. 6: commands are unique, so a double execution is one server's sequence repeating a slot;
// seed 4420464 forks at 78, after which the observer's stitched order held a repeat at 92.
#[test]
fn exec_dup_on_seed_4420464_matches_per_server_repeats() {
    let sc = recovery_r4(4420464, 130, "skip_boundary", "skip");
    let (report, gt) = run_with_ground_truth(&sc);
    assert!(!report.safety_ok, "precondition: the run forks");
    assert_eq!(report.exec_dup_round, gt.first_repeat);
    assert_eq!(run_smr_grid(&sc).exec_dup_round, gt.first_repeat);
}

// p. 6: the double-execution round is the round a server first repeats a slot.
#[test]
fn exec_dup_on_seed_5000532_is_dated_to_the_first_real_repeat() {
    let (report, gt) =
        run_with_ground_truth(&recovery_r4(5000532, 130, "skip_boundary", "execute"));
    assert!(!report.safety_ok, "precondition: the run forks");
    assert_eq!(report.exec_dup_round, gt.first_repeat);
}

// p. 6: a real repeat on prefix-consistent servers (skipping alone re-commits a command).
#[test]
fn exec_dup_on_seed_4330140_is_the_first_real_repeat() {
    let sc = scenario(serde_json::json!({
        "n": 32, "seed": 4330140, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": 26, "resend_until_acked": false,
                  "prefix_mismatch": "skip_boundary"},
        "repeated_commit": "execute", "injections": [], "max_rounds": 260,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
    }));
    let (report, gt) = run_with_ground_truth(&sc);
    assert!(gt.first_repeat.is_some(), "precondition: a server repeats");
    assert_eq!(report.exec_dup_round, gt.first_repeat);
    assert_eq!(run_smr_grid(&sc).exec_dup_round, gt.first_repeat);
}

// Lemma 6.8, pp. 35-36: executed_round = r only if every log-holding server executed the command.
#[test]
fn executed_round_requires_every_log_holding_server_to_have_executed() {
    let s = recovery_r4(4420314, 131, "skip_boundary", "skip");
    let mut at_130 = None;
    let report = run_smr_observed(&s, &mut |st: &SmrState| {
        if st.round() == 130 {
            let holds: Vec<bool> = (0..s.n)
                .map(|i| st.rec_node_detail(i).expect("node").log_len.is_some())
                .collect();
            at_130 = Some((holds, st.executed_seqs()));
        }
    });
    let (holds, seqs) = at_130.expect("round 130 observed");
    let offenders: Vec<(u64, Vec<usize>)> = report
        .commands
        .iter()
        .filter(|c| c.executed_round == Some(130))
        .filter_map(|c| {
            let missing: Vec<usize> = (0..s.n)
                .filter(|&i| {
                    holds[i]
                        && !seqs[i]
                            .iter()
                            .any(|e| e.client_sn().map(|(cl, _)| cl) == Some(c.client))
                })
                .collect();
            (!missing.is_empty()).then_some((c.op, missing))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "commands marked executed at round 130 that log-holding servers never executed: \
         {offenders:?}"
    );
}

// Lemma 6.9, p. 36: an executed-length regression ends the run as a recorded failure.
#[test]
fn executed_length_regression_is_reported_not_a_panic() {
    let s = burst_scenario(9, PrefixMismatch::SkipBoundary);
    let report = catch_unwind(AssertUnwindSafe(|| run_smr(&s)))
        .unwrap_or_else(|_| panic!("run_smr panicked instead of recording the regression"));
    let SmrTerminal::Failed {
        attempted_round,
        phase: RunFailurePhase::ExecutedRegression,
        ..
    } = report.terminal
    else {
        panic!("precondition: this seed regresses ({:?})", report.terminal)
    };
    let failure = report.failure.as_ref().expect("typed failure");
    let RecoveryViolation::ExecutedRegression { previous_len, len } = failure.violation else {
        panic!("a regression: {:?}", failure.violation)
    };
    assert!(len < previous_len);
    assert!(report.recovery.is_some());
    assert_eq!(
        report.metrics.len(),
        attempted_round,
        "the failed round is kept"
    );
    assert_eq!(
        run_smr_grid(&s).terminal,
        report.terminal,
        "lean and full record the same outcome"
    );
}

// p. 29: a reachable boundary fork is latched as safety_ok = false in every build profile.
#[test]
fn boundary_fork_latches_safety_instead_of_panicking_in_debug_builds() {
    let s = burst_scenario(9, PrefixMismatch::Abort);
    let report = catch_unwind(AssertUnwindSafe(|| run_smr(&s)))
        .unwrap_or_else(|_| panic!("run_smr panicked instead of latching the fork"));
    assert!(!report.safety_ok, "the boundary fork must be latched");
}
