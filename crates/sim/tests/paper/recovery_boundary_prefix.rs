use sim::smr::{
    ClientModel, PrefixMismatch, Proto, RecoveryViolation, SmrScenario, TrafficPhase, run_smr,
};
use sim::{BlockSchedule, Config};

/// n = 64, T = 10, beta 0.1 fresh, two arrivals per round: B-2a stamp drift shows by round 30.
fn stamp_drift(seed: u64) -> SmrScenario {
    SmrScenario {
        n: 64,
        seed,
        cfg: Config::new(6, 3).expect("valid config"),
        sigma: 1.0,
        proto: Proto::Recovery {
            t_window_rounds: 10,
            resend_until_acked: false,
            prefix_mismatch: PrefixMismatch::Abort,
        },
        injections: Vec::new(),
        max_rounds: 40,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.0, 0.0, 1.0],
        }]),
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

// Alg 6 B-2a, p. 31: P is a sequence of commands, so a boundary abort names a command mismatch.
#[test]
fn a_boundary_abort_is_never_a_stamp_only_difference() {
    for seed in 0..12 {
        let report = run_smr(&stamp_drift(seed));
        let Some(failure) = &report.failure else {
            continue;
        };
        let RecoveryViolation::Prefix(v) = &failure.violation else {
            continue;
        };
        let expected = v.expected.as_ref().map(|t| &t.entry);
        let actual = v.actual.as_ref().map(|t| &t.entry);
        assert_ne!(
            expected, actual,
            "seed {seed}: node {} aborted at round {} on a stamp-only difference: {v}",
            failure.node, failure.attempted_round
        );
    }
}
