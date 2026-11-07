// The opt-in execute-once state machine needs sn-ordered commits; a commit that skips a client's
// sequence number ends the run as a recorded failure instead of a panic.

use sim::smr::{
    RecoveryViolation, RunFailurePhase, SmrReport, SmrScenario, SmrTerminal, run_smr, run_smr_grid,
    run_smr_lean,
};
use sim::spec::SmrScenarioSpec;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn scenario(json: serde_json::Value) -> SmrScenario {
    let spec: SmrScenarioSpec = serde_json::from_value(json).expect("spec parses");
    SmrScenario::try_from(spec).expect("valid scenario")
}

// Pool clients under fresh blocking: one node commits a client's sn s + 1 before its sn s.
fn compact_pool(seed: u64, repeated_commit: &str) -> SmrScenario {
    scenario(serde_json::json!({
        "n": 16, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "compact", "t_commit_rounds": 2},
        "injections": [], "max_rounds": 300,
        "schedule": {"kind": "fresh_per_round", "fraction": 0.2},
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 1.0]},
        "client_model": {"kind": "pool", "clients": 24},
        "repeated_commit": repeated_commit,
    }))
}

// A paced ack-mode pool surge under skip_boundary: a forked server commits a client's sn 3 before its sn 2.
fn recovery_surge(seed: u64, repeated_commit: &str) -> SmrScenario {
    scenario(serde_json::json!({
        "n": 32, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {
            "kind": "recovery", "t_window_rounds": 8, "resend_until_acked": true,
            "prefix_mismatch": "skip_boundary"
        },
        "injections": [], "max_rounds": 150,
        "schedule": {"kind": "fresh_per_round", "fraction": 0.1},
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]},
        "client_model": {"kind": "pool", "clients": 1000},
        "repeated_commit": repeated_commit,
    }))
}

fn run_without_panic(run: fn(&SmrScenario) -> SmrReport, s: &SmrScenario) -> SmrReport {
    catch_unwind(AssertUnwindSafe(|| run(s)))
        .unwrap_or_else(|_| panic!("the Skip run panicked instead of reporting"))
}

fn assert_sn_order_failure(report: &SmrReport, client: u32) {
    let SmrTerminal::Failed {
        attempted_round,
        completed_round,
        observed_round,
        phase: RunFailurePhase::OutOfSnOrderCommit,
    } = report.terminal
    else {
        panic!("expected an out-of-sn-order failure: {:?}", report.terminal)
    };
    assert_eq!(completed_round + 1, attempted_round);
    assert_eq!(observed_round, attempted_round);
    let failure = report.failure.as_ref().expect("typed failure");
    let RecoveryViolation::OutOfSnOrder {
        client: c,
        sn,
        committed_sn,
    } = failure.violation
    else {
        panic!("an sn-order violation: {:?}", failure.violation)
    };
    assert_eq!(c, client);
    assert!(
        sn > committed_sn + 1,
        "sn {sn} does not skip past {committed_sn}"
    );
    assert_eq!(
        report.metrics.len(),
        attempted_round,
        "the failed round is kept"
    );
}

#[test]
fn compact_skip_out_of_sn_order_commit_is_a_recorded_failure() {
    let s = compact_pool(98, "skip");
    let report = run_without_panic(run_smr_lean, &s);
    let client = match report.failure.as_ref().map(|f| &f.violation) {
        Some(RecoveryViolation::OutOfSnOrder { client, .. }) => *client,
        other => panic!("precondition: seed 98 commits out of sn order ({other:?})"),
    };
    assert_sn_order_failure(&report, client);
    assert_eq!(
        run_without_panic(run_smr, &s).terminal,
        report.terminal,
        "lean and full record the same outcome"
    );
}

#[test]
fn compact_execute_never_fails_on_the_same_seed() {
    let report = run_smr_lean(&compact_pool(98, "execute"));
    assert!(!report.terminal.is_failed(), "{:?}", report.terminal);
}

#[test]
fn recovery_skip_out_of_sn_order_commit_is_a_recorded_failure() {
    let s = recovery_surge(25, "skip");
    let report = run_without_panic(run_smr_grid, &s);
    let client = match report.failure.as_ref().map(|f| &f.violation) {
        Some(RecoveryViolation::OutOfSnOrder { client, .. }) => *client,
        other => panic!("precondition: seed 25 commits out of sn order ({other:?})"),
    };
    assert_sn_order_failure(&report, client);
    assert!(report.recovery.is_some());
    assert_eq!(
        run_without_panic(run_smr, &s).terminal,
        report.terminal,
        "lean and full record the same outcome"
    );
}

#[test]
fn recovery_execute_never_fails_on_the_same_seed() {
    let report = run_smr_grid(&recovery_surge(25, "execute"));
    assert!(!report.terminal.is_failed(), "{:?}", report.terminal);
}
