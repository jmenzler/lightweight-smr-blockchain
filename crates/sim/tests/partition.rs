//! Network partition: a component id per node, gating every server-to-server
//! reply. Nodes do not know the partition, so target SAMPLING stays
//! unrestricted — a cross-component target is drawn and simply never answers.

use sim::smr::{ClientModel, Injection, Proto, SmrScenario, SmrState, SmrStepStatus};
use sim::spec::{InitSpec, ProtoSpec, ScenarioSpec, SmrScenarioSpec};
use sim::{BlockSchedule, BlockTarget, BlockWindow, Config, Init, Scenario, run, run_traced};

/// Contiguous two-block partition: the first `first` ids in component 0, the
/// rest in component 1.
fn two_blocks(first: usize, n: usize) -> Vec<u32> {
    (0..n).map(|i| u32::from(i >= first)).collect()
}

fn alg1(n: usize, useful: f64, partition: Option<Vec<u32>>) -> Scenario {
    Scenario {
        n,
        seed: 6_000_000,
        cfg: Config::default(),
        init: Init::WithUndecided {
            useful_fraction: useful,
            inner: Box::new(Init::Split { fraction: 0.5 }),
        },
        max_rounds: 40,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.0 },
        partition,
    }
}

/// The median rule never invents a value, so a component whose members all
/// start ⊥ can only ever be revived from outside — which is exactly what the
/// partition forbids.
#[test]
fn an_all_bottom_component_never_acquires_a_value() {
    let n = 200;
    let scenario = alg1(n, 0.8, Some(two_blocks(160, n)));
    let (_, trace) = run_traced(&scenario);

    for (r, round) in trace.rounds.iter().enumerate() {
        let revived: Vec<usize> = (160..n).filter(|&i| round.states[i].is_some()).collect();
        assert!(
            revived.is_empty(),
            "round {}: isolated ⊥ nodes {revived:?} acquired a value across the partition",
            r + 1
        );
    }
    let live = trace.rounds.last().expect("ran at least one round");
    assert!(
        (0..160).any(|i| live.states[i].is_some()),
        "the 0.8 component must stay live, else the assertion above is vacuous"
    );
}

/// The gate is a condition on an already-drawn target, not a redraw: the k
/// target ids of round 1 are the same partitioned or not.
#[test]
fn a_partition_leaves_the_target_draws_untouched() {
    let n = 200;
    let (_, open) = run_traced(&alg1(n, 0.8, None));
    let (_, split) = run_traced(&alg1(n, 0.8, Some(two_blocks(160, n))));

    assert_eq!(
        open.rounds[0].targets, split.rounds[0].targets,
        "partitioning must not move the sampling stream"
    );
    assert_ne!(
        open.rounds[0].states, split.rounds[0].states,
        "…while the replies those targets give must differ"
    );
}

/// Cross-component targets never reply, so all nodes holding a value is not absorbing under a
/// partition: even without blocking, a run must not stop at its first all-hold round.
#[test]
fn an_all_hold_round_is_not_absorbing_under_a_partition() {
    let run_with = |seed: u64, schedule: BlockSchedule| {
        run(&Scenario {
            n: 4,
            seed,
            cfg: Config::default(),
            init: Init::UniformRandom { k: 1 },
            max_rounds: 60,
            schedule,
            partition: Some(two_blocks(2, 4)),
        })
    };
    // A window that opens after the horizon blocks nothing and draws nothing, but keeps
    // done_blocking_after false, so it never takes the early exit.
    let never_exits_early = || {
        BlockSchedule::Windows(vec![BlockWindow {
            start_round: 1_000_000,
            rounds: 1,
            target: BlockTarget::Nodes(vec![0]),
        }])
    };
    for seed in 0..300 {
        assert_eq!(
            run_with(seed, BlockSchedule::FreshPerRound { fraction: 0.0 }),
            run_with(seed, never_exits_early()),
            "seed {seed}"
        );
    }
}

#[test]
fn a_partition_must_name_every_node() {
    let spec = ScenarioSpec {
        n: 8,
        seed: 1,
        k: 6,
        ell: 3,
        init: InitSpec::Split { fraction: 0.5 },
        max_rounds: 10,
        schedule: None,
        partition: Some(vec![0, 0, 0, 1, 1]),
    };
    let err = Scenario::try_from(spec).expect_err("length mismatch must be rejected");
    assert!(err.contains("partition"), "unexpected error: {err}");
}

#[test]
fn an_smr_partition_must_name_every_node() {
    let spec = SmrScenarioSpec {
        n: 8,
        seed: 1,
        k: 6,
        ell: 3,
        sigma: 5.0,
        proto: ProtoSpec::Extended,
        injections: vec![],
        max_rounds: 10,
        schedule: None,
        traffic: None,
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: vec![],
        partition: Some(vec![0; 9]),
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    };
    let err = SmrScenario::try_from(spec).expect_err("length mismatch must be rejected");
    assert!(err.contains("partition"), "unexpected error: {err}");
}

const SMR_N: usize = 64;
const SMR_FIRST: usize = 48;

/// One command pinned into component 0, stepped with nobody blocked; the
/// horizon stays inside `t_commit`/`t_window` so no engine ever strips it out
/// of a log and the holder set only grows.
fn spread_under_partition(proto: Proto) -> Vec<SmrStepStatus> {
    let mut state = SmrState::new(
        SMR_N,
        Config::default(),
        proto,
        5.0,
        6_000_001,
        &[Injection {
            round: 1,
            client: 1,
            op: 7,
            target: Some(0),
        }],
        &[],
        ClientModel::Unique,
        Some(&two_blocks(SMR_FIRST, SMR_N)),
    );
    let mask = vec![false; SMR_N];
    (0..25).map(|_| state.step_masked(&mask)).collect()
}

fn assert_contained(statuses: &[SmrStepStatus], label: &str) {
    let mut peak = 0;
    for status in statuses {
        for cmd in &status.spreading {
            peak = peak.max(cmd.holders.len());
            let leaked: Vec<u32> = cmd
                .holders
                .iter()
                .copied()
                .filter(|&id| id as usize >= SMR_FIRST)
                .collect();
            assert!(
                leaked.is_empty(),
                "{label} round {}: command crossed the partition to {leaked:?}",
                status.round
            );
        }
    }
    assert!(
        peak >= 5,
        "{label}: the command must still spread inside its own component (peak {peak})"
    );
}

#[test]
fn an_extended_command_stays_inside_its_component() {
    assert_contained(&spread_under_partition(Proto::Extended), "extended");
}

#[test]
fn a_compact_command_stays_inside_its_component() {
    assert_contained(
        &spread_under_partition(Proto::Compact {
            t_commit_rounds: 100,
        }),
        "compact",
    );
}

#[test]
fn a_recovery_command_stays_inside_its_component() {
    assert_contained(
        &spread_under_partition(Proto::Recovery {
            t_window_rounds: 100,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        }),
        "recovery",
    );
}
