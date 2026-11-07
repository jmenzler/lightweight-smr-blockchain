//! Lemma 6.7 (p. 35) on the commit path: servers about to commit at a window end from
//! checkpoints of one W that differ at command level. A rollback (R_i = reset) commits the
//! server's own P, which prefixes the L_i it restores, so the node-local boundary check cannot
//! see this; the population-level check dates it under both mismatch policies and stops neither.

use sim::smr::{
    ClientModel, ManualBlock, PrefixMismatch, Proto, RecoveryViolation, RunFailurePhase,
    SmrScenario, point_mass_pmf,
};
use sim::{BlockSchedule, Config};

/// n = 32, T = 15, four arrivals a round, every server blocked in rounds 37-38.
fn surge(seed: u64, prefix_mismatch: PrefixMismatch) -> SmrScenario {
    SmrScenario {
        n: 32,
        seed,
        cfg: Config::new(6, 3).expect("valid config"),
        sigma: 1.0,
        proto: Proto::Recovery {
            t_window_rounds: 15,
            resend_until_acked: false,
            prefix_mismatch,
        },
        injections: Vec::new(),
        max_rounds: 140,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![sim::smr::TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(4),
        }]),
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: (0..32)
            .map(|node| ManualBlock {
                node,
                from_round: 37,
                to_round: Some(38),
            })
            .collect(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

// Lemma 6.7, pp. 31/35: the date is a report, not a stop. Under Abort the preflight stops only
// on a node-local prefix mismatch (which can fall on the dated window end), so a rollback
// commits the forked checkpoint and the run goes on.
#[test]
fn under_abort_a_dated_checkpoint_fork_does_not_stop_the_run() {
    let mut ran_past = 0;
    for seed in 0..30 {
        let report = sim::smr::run_smr(&surge(seed, PrefixMismatch::Abort));
        let Some(dated) = report
            .recovery
            .as_ref()
            .expect("recovery")
            .commit_fork_round
        else {
            continue;
        };
        match &report.failure {
            Some(failure) if failure.attempted_round <= dated => assert!(
                matches!(failure.violation, RecoveryViolation::Prefix(_)),
                "seed {seed}: the run stopped at the dated window end {dated}: {failure:?}"
            ),
            _ => ran_past += 1,
        }
    }
    assert!(
        ran_past > 0,
        "precondition: some Abort run dates a fork and runs past it"
    );
}

// Both policies run identically up to the first node-local prefix mismatch, which ends an Abort
// run, so up to that round both date the same window end.
#[test]
fn the_commit_path_checkpoint_fork_is_dated_identically_under_both_policies() {
    let mut matched = 0;
    for seed in 0..30 {
        let abort = sim::smr::run_smr(&surge(seed, PrefixMismatch::Abort));
        let skip = sim::smr::run_smr(&surge(seed, PrefixMismatch::SkipBoundary));
        let dated =
            |r: &sim::smr::SmrReport| r.recovery.as_ref().expect("recovery").commit_fork_round;
        let cut = abort.failure.as_ref().map(|f| f.attempted_round);
        if let Some(failure) = &abort.failure {
            assert_eq!(failure.phase, RunFailurePhase::BoundaryPreflight);
            assert!(matches!(failure.violation, RecoveryViolation::Prefix(_)));
        }
        assert_eq!(
            dated(&abort),
            dated(&skip).filter(|&r| cut.is_none_or(|c| r <= c)),
            "seed {seed}"
        );
        matched += usize::from(dated(&abort).is_some());
    }
    assert!(matched > 0, "precondition: some Abort run dates a fork");
}

// The date reaches the grid CSV and the ledger, read off the report.
#[test]
fn the_commit_fork_round_reaches_the_grid_row_and_the_ledger() {
    let seed = (0..30)
        .find(|&s| {
            sim::smr::run_smr_grid(&surge(s, PrefixMismatch::SkipBoundary))
                .recovery
                .and_then(|r| r.commit_fork_round)
                .is_some()
        })
        .expect("precondition: a SkipBoundary run dates a fork");
    let blocks: Vec<serde_json::Value> = (0..32)
        .map(|node| serde_json::json!({"node": node, "from_round": 37, "to_round": 38}))
        .collect();
    let spec: sim::sweep::SmrGridSpec = serde_json::from_value(serde_json::json!({
        "exp": "commit-fork",
        "base": {
            "n": 32, "seed": 0, "k": 6, "ell": 3, "sigma": 1.0,
            "proto": {"kind": "recovery", "t_window_rounds": 15,
                      "prefix_mismatch": "skip_boundary", "resend_until_acked": false},
            "injections": [], "max_rounds": 140,
            "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
            "schedule": {"kind": "fresh_per_round", "fraction": 0.1},
            "manual_blocks": blocks,
        },
        "ladder": [{"n": 32, "seed_base": seed, "seed_count": 1}],
    }))
    .expect("spec parses");
    let (_, scenario) = sim::sweep::expand_smr(&spec).expect("valid").remove(0);
    let want = sim::smr::run_smr_grid(&scenario)
        .recovery
        .and_then(|r| r.commit_fork_round)
        .expect("dated");
    let dir = std::env::temp_dir().join(format!("commit-fork-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ledger = dir.join("ledger.jsonl");
    let csv = sim::sweep::run_smr_grid_streaming(&spec, Some(&ledger), None).expect("grid runs");
    let header: Vec<&str> = csv.lines().next().unwrap().split(',').collect();
    let row: Vec<&str> = csv.lines().nth(1).unwrap().split(',').collect();
    let at = header
        .iter()
        .position(|h| *h == "commit_fork_round")
        .unwrap();
    assert_eq!(row[at], want.to_string());
    let line = std::fs::read_to_string(&ledger).unwrap();
    let record: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
    assert_eq!(record["outcome"]["recovery_commit_fork_round"], want);
    std::fs::remove_dir_all(&dir).ok();
}

// Lemma 6.7, p. 35: two checkpoints of one W are a commit fork only once both are committed. In
// seed 14 the first-seen class of W = 6 fails its boundary check at 91 and never commits.
#[test]
fn a_window_whose_other_checkpoint_is_never_committed_is_no_commit_fork() {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(serde_json::json!({
        "n": 32, "seed": 14, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": 13, "resend_until_acked": false},
        "injections": [], "max_rounds": 130,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": 0.1},
    }))
    .expect("spec parses");
    let report = sim::smr::run_smr(&SmrScenario::try_from(spec).expect("valid scenario"));
    let rec = report.recovery.as_ref().expect("recovery");
    assert!(
        !rec.fork_ok,
        "precondition: two checkpoints of one W exist ({:?})",
        report.terminal
    );
    assert_eq!(rec.commit_fork_round, None);
}
