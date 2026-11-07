use protocol::Config;
use sim::{BlockSchedule, BlockTarget, BlockWindow, Init, Outcome, Scenario, run, run_traced};

fn base(n: usize, seed: u64, init: Init) -> Scenario {
    Scenario {
        n,
        seed,
        cfg: Config::default(),
        init,
        max_rounds: 1000,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.0 },
        partition: None,
    }
}

fn blocked(n: usize, seed: u64, schedule: BlockSchedule, max_rounds: usize) -> Scenario {
    Scenario {
        n,
        seed,
        cfg: Config::default(),
        init: Init::Split { fraction: 0.5 },
        max_rounds,
        schedule,
        partition: None,
    }
}

#[test]
fn scenario_config_drives_the_fanout() {
    let mut s = base(50, 11, Init::Split { fraction: 0.5 });
    s.cfg = Config::new(8, 5).unwrap();
    let (_, trace) = run_traced(&s);
    for round in &trace.rounds {
        for targets in &round.targets {
            assert_eq!(targets.len(), 8, "each node samples k=8 targets");
        }
    }
}

#[test]
fn converges_to_agreement_from_random_init() {
    let out = run(&base(100, 1, Init::UniformRandom { k: 5 }));
    match out {
        Outcome::Agreement { rounds, .. } => assert!(rounds > 0),
        other => panic!("expected agreement, got {other:?}"),
    }
}

#[test]
fn same_seed_same_outcome() {
    let a = run(&base(200, 7, Init::Split { fraction: 0.5 }));
    let b = run(&base(200, 7, Init::Split { fraction: 0.5 }));
    assert_eq!(
        a, b,
        "determinism: identical seed must replay byte-identically"
    );
}

#[test]
fn different_seeds_may_differ_but_both_agree() {
    let a = run(&base(100, 1, Init::Split { fraction: 0.5 }));
    let b = run(&base(100, 2, Init::Split { fraction: 0.5 }));
    assert!(matches!(a, Outcome::Agreement { .. }));
    assert!(matches!(b, Outcome::Agreement { .. }));
}

#[test]
fn agreed_value_is_one_of_the_inputs() {
    // validity end-to-end: decided value must be some node's input
    for seed in 0..5 {
        let out = run(&base(50, seed, Init::UniformRandom { k: 3 }));
        match out {
            Outcome::Agreement { value, .. } => assert!(value < 3, "value {value} not an input"),
            other => panic!("expected agreement, got {other:?}"),
        }
    }
}

#[test]
fn distinct_inputs_still_converge() {
    let out = run(&base(100, 3, Init::Distinct));
    assert!(matches!(out, Outcome::Agreement { .. }));
}

#[test]
fn window_blocks_only_inside_its_rounds() {
    // FT-1/FT-2: block(at_round=5, for_rounds=3) — the control-plane adversary
    let schedule = BlockSchedule::Windows(vec![BlockWindow {
        start_round: 5,
        rounds: 3,
        target: BlockTarget::SampleFraction(0.2),
    }]);
    let (_, trace) = run_traced(&blocked(100, 17, schedule, 12));
    for (i, round) in trace.rounds.iter().enumerate() {
        let r = i + 1;
        if (5..=7).contains(&r) {
            assert_eq!(round.blocked.len(), 20, "round {r}: 20% of 100 blocked");
        } else {
            assert!(round.blocked.is_empty(), "round {r}: no window active");
        }
    }
}

#[test]
fn overlapping_windows_block_the_union() {
    let schedule = BlockSchedule::Windows(vec![
        BlockWindow {
            start_round: 1,
            rounds: 10,
            target: BlockTarget::Nodes(vec![0, 1, 2]),
        },
        BlockWindow {
            start_round: 5,
            rounds: 6,
            target: BlockTarget::Nodes(vec![2, 3]),
        },
    ]);
    let (_, trace) = run_traced(&blocked(50, 3, schedule, 10));
    assert_eq!(
        trace.rounds[1].blocked,
        vec![0, 1, 2],
        "round 2: first only"
    );
    assert_eq!(
        trace.rounds[5].blocked,
        vec![0, 1, 2, 3],
        "round 6: union, no duplicate for node 2"
    );
}

#[test]
fn windows_past_make_unanimity_absorbing_again() {
    // After the last window ends nobody can be blocked, so once every node
    // holds the agreed value the run exits instead of simulating to max_rounds.
    let schedule = BlockSchedule::Windows(vec![BlockWindow {
        start_round: 1,
        rounds: 2,
        target: BlockTarget::SampleFraction(0.3),
    }]);
    let (out, trace) = run_traced(&blocked(100, 8, schedule, 1000));
    assert!(matches!(out, Outcome::Agreement { .. }), "got {out:?}");
    assert!(
        trace.rounds.len() < 1000,
        "exit early once windows are past and all nodes hold"
    );
}

fn with_undecided(useful_fraction: f64, inner: Init) -> Init {
    Init::WithUndecided {
        useful_fraction,
        inner: Box::new(inner),
    }
}

#[test]
fn subcritical_useful_fraction_death_spirals_without_blocking() {
    // Lemma 2.1 regime: useful < 1/3 − ε kills the population with ZERO blocking
    let init = with_undecided(0.30, Init::Split { fraction: 0.5 });
    let out = run(&base(1000, 21, init));
    assert!(matches!(out, Outcome::AllUndecided { .. }), "got {out:?}");
}

#[test]
fn half_useful_start_without_blocking_recovers_and_agrees() {
    // Lemma 2.2 starting condition (zero-blocking case): ≥ 1/2 useful recovers
    let init = with_undecided(0.5, Init::Split { fraction: 0.5 });
    let out = run(&base(1000, 22, init));
    assert!(matches!(out, Outcome::Agreement { .. }), "got {out:?}");
}

#[test]
fn unanimity_among_holders_is_not_absorbing_while_undecided_remain() {
    // Half the nodes start ⊥, all holders share value 0: holders are unanimous
    // from the start, yet each can still die by sampling a ⊥-heavy set — the
    // run must not exit until no ⊥ nodes remain (the Lemma 2.1 mechanism).
    let init = with_undecided(0.5, Init::Split { fraction: 1.0 });
    let (out, trace) = run_traced(&base(200, 13, init));
    assert!(matches!(out, Outcome::Agreement { value: 0, .. }));
    let last = &trace.rounds.last().unwrap().states;
    assert!(
        last.iter().all(|s| s.is_some()),
        "exited while ⊥ nodes could still kill the holders"
    );
}

#[test]
fn transient_unanimity_during_collapse_is_not_survival() {
    // At 30% blocking the population is subcritical (E[replies] < ell) and dies.
    // Mid-collapse the few surviving holders transiently share one value: that is
    // Def 1.2 agreement (on a vanishing population), but availability is lost.
    let out = run(&blocked(
        1000,
        49,
        BlockSchedule::FreshPerRound { fraction: 0.30 },
        200,
    ));
    assert!(!out.survived(), "got {out:?}");
    assert_eq!(out.extinction_round(), Some(6), "got {out:?}");
}

#[test]
fn total_blocking_causes_death_spiral() {
    // every node blocked every round -> nobody ever gets replies -> all undecided
    let out = run(&blocked(
        50,
        1,
        BlockSchedule::FreshPerRound { fraction: 1.0 },
        100,
    ));
    assert_eq!(out, Outcome::AllUndecided { rounds: 1 });
}

#[test]
fn permanent_schedule_blocks_the_same_nodes_every_round() {
    let (_, trace) = run_traced(&blocked(
        200,
        3,
        BlockSchedule::Permanent { fraction: 0.2 },
        50,
    ));
    let first = trace.rounds[0].blocked.clone();
    assert_eq!(first.len(), 40, "20% of 200 nodes blocked");
    for round in &trace.rounds {
        assert_eq!(round.blocked, first);
    }
}

#[test]
fn permanent_blocking_at_three_tenths_death_spirals() {
    // Lemma 2.3 regime: >= 3/10 permanently blocked kills the population
    let out = run(&blocked(
        1000,
        1,
        BlockSchedule::Permanent { fraction: 0.30 },
        500,
    ));
    assert!(out.extinction_round().is_some(), "got {out:?}");
}

#[test]
fn permanent_blocking_at_two_tenths_survives() {
    // below the permanent-schedule mean-field boundary (~0.269): stays live
    let out = run(&blocked(
        1000,
        1,
        BlockSchedule::Permanent { fraction: 0.20 },
        500,
    ));
    assert!(matches!(out, Outcome::Agreement { .. }), "got {out:?}");
}

#[test]
fn even_split_values_match_exact_arithmetic_where_the_product_exceeds_32_bits() {
    // i · values overflows a 32-bit usize (the wasm32 target) from n = values = 65 536 on; the
    // initial value of server i must still be ⌊i · values / n⌋ computed without overflow.
    use rand::SeedableRng;
    let (n, values) = (70_000usize, 70_000usize);
    let mut rng = rand_chacha::ChaCha12Rng::seed_from_u64(0);
    for i in [0usize, 1, 65_535, 65_536, 69_999] {
        let exact = (i as u128 * values as u128 / n as u128) as u64;
        assert_eq!(
            sim::initial_state(&Init::EvenSplit { values }, i, n, &mut rng),
            Some(exact),
            "server {i}"
        );
    }
}
