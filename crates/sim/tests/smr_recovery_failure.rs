use sim::smr::{
    ClientModel, CommandStatus, Injection, Proto, RecoveryViolation, RunFailurePhase, SmrReport,
    SmrScenario, SmrState, SmrTerminal, Spill, TrafficPhase, run_smr, run_smr_lean,
    run_smr_observed,
};
use sim::{BlockSchedule, Config};

fn failing_scenario() -> SmrScenario {
    SmrScenario {
        n: 598,
        seed: 1,
        cfg: Config::new(6, 3).expect("valid config"),
        sigma: 1.0,
        proto: Proto::Recovery {
            t_window_rounds: 20,
            resend_until_acked: true,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        injections: vec![
            Injection {
                round: 2,
                client: 1,
                op: 1,
                target: None,
            },
            Injection {
                round: 2,
                client: 2,
                op: 2,
                target: None,
            },
        ],
        max_rounds: 200,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.0, 0.0, 1.0],
        }]),
        client_model: ClientModel::Unique,
        certs: true,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

fn failure_rounds(report: &SmrReport) -> (usize, usize, usize) {
    match report.terminal {
        SmrTerminal::Failed {
            attempted_round,
            completed_round,
            observed_round,
            phase: RunFailurePhase::BoundaryPreflight,
        } => (attempted_round, completed_round, observed_round),
        ref terminal => panic!("expected boundary-preflight failure, got {terminal:?}"),
    }
}

#[test]
fn boundary_prefix_violation_becomes_a_typed_terminal_failure() {
    let report = run_smr(&failing_scenario());
    let (attempted, completed, observed) = failure_rounds(&report);
    assert_eq!(attempted % 20, 0, "failure must occur at a boundary");
    assert_eq!(completed + 1, attempted);
    assert_eq!(observed, attempted);
    assert_eq!(report.metrics.len(), attempted, "failed round is retained");
    assert_eq!(
        report.recovery.as_ref().expect("recovery").rounds.len(),
        attempted
    );
    let failure = report.failure.as_ref().expect("typed failure");
    assert_eq!(failure.node, 1);
    let RecoveryViolation::Prefix(violation) = &failure.violation else {
        panic!("a prefix violation: {:?}", failure.violation)
    };
    assert_eq!(violation.pre_len, 39);
    assert_eq!(violation.log_len, 114);
    assert_eq!(violation.first_mismatch, 33);
    assert_ne!(
        violation.expected.as_ref().map(|t| &t.entry),
        violation.actual.as_ref().map(|t| &t.entry),
        "a boundary failure is a command mismatch"
    );
    assert_eq!(report.recovered_round(598, 20), None);
    assert!(
        report
            .commands
            .iter()
            .any(|c| c.status == CommandStatus::Complete)
    );
    assert!(
        report
            .commands
            .iter()
            .any(|c| c.status == CommandStatus::Pending)
    );
}

#[test]
fn failed_round_is_reported_but_not_sent_to_the_external_observer() {
    let mut observed_rounds = Vec::new();
    let report = run_smr_observed(&failing_scenario(), &mut |state| {
        observed_rounds.push(state.round());
    });
    let (attempted, completed, observed) = failure_rounds(&report);
    assert_eq!(observed_rounds, (1..=completed).collect::<Vec<_>>());
    assert_eq!(report.metrics.len(), attempted);
    assert_eq!(observed, attempted);
}

#[test]
fn failed_lean_run_retains_a_parseable_unfinished_spill() {
    const CHILD: &str = "RECOVERY_FAILURE_SPILL_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "failed_lean_run_retains_a_parseable_unfinished_spill",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env_remove("SIM_RUNLOG")
            .env_remove("SIM_SPILL")
            .output()
            .expect("spawn spill-enabled child");
        assert!(
            output.status.success(),
            "spill-enabled child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let report = run_smr_lean(&failing_scenario());
    let (attempted, _, _) = failure_rounds(&report);
    let path = report
        .spill_path
        .as_deref()
        .expect("failed lean spill path");
    let text = std::fs::read_to_string(path).expect("flushed spill records");
    let spill = Spill::parse(&text).expect("prior records remain parseable");
    assert_eq!(
        spill.boundaries.iter().map(|b| b.round).collect::<Vec<_>>(),
        (20..attempted).step_by(20).collect::<Vec<_>>()
    );
    assert!(
        spill.trailer.is_none(),
        "failed run must not have a success trailer"
    );
    std::fs::remove_file(path).expect("remove test spill");
}

#[test]
fn failed_state_is_absorbing_and_reuses_the_cached_status() {
    let scenario = failing_scenario();
    let mut state = SmrState::new_with_certs(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &scenario.injections,
        scenario.traffic.as_deref().unwrap_or(&[]),
        scenario.client_model,
        scenario.partition.as_deref(),
        scenario.certs,
        scenario.merge_policy,
        scenario.repeated_commit,
    );
    let mut failed = None;
    for _ in 1..=scenario.max_rounds {
        state.draw_arrivals();
        let status = state.step_fraction(0.1);
        if status.failed() {
            failed = Some(status);
            break;
        }
    }
    let failed = failed.expect("fixture reaches a failed boundary");
    let failed_round = state.round();
    assert_eq!(failed_round % 20, 0, "failure must occur at a boundary");
    assert_eq!(state.failure(), failed.failure.as_ref());
    assert_eq!(state.step_masked(&vec![false; scenario.n]), failed);
    assert_eq!(
        state.round(),
        failed_round,
        "cached failure does not advance"
    );
    assert!(state.inject(9, 99, None).is_err());
    assert!(state.set_traffic_pmf(vec![1.0]).is_err());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.draw_arrivals())).is_err()
    );
    assert!(std::panic::catch_unwind(|| state.rec_node_detail(0)).is_err());
}
