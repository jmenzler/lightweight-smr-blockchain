use super::*;
use rand::SeedableRng;

#[test]
fn schedule_matrix_equals_driver_draw_sequence() {
    let n = 12;
    let rounds = 15;
    let variants: Vec<BlockSchedule> = vec![
        BlockSchedule::FreshPerRound { fraction: 0.25 },
        BlockSchedule::Permanent { fraction: 0.5 },
        BlockSchedule::Windows(vec![crate::BlockWindow {
            start_round: 3,
            rounds: 4,
            target: BlockTarget::SampleFraction(0.25),
        }]),
        BlockSchedule::PerRoundFractions(vec![0.5, 0.0, 0.25]),
        BlockSchedule::PerRoundSticky {
            background: 0.1,
            targets: vec![0.25, 0.25, 0.5, 0.0],
        },
    ];
    for sched in variants {
        let got = mask_schedule(&sched, n, rounds, &mut ChaCha12Rng::seed_from_u64(42));
        let mut rng = ChaCha12Rng::seed_from_u64(42);
        let mut blocking = compile_blocking(&sched, n, &mut rng);
        let delay = SnapshotDelay::new(blocking.knowledge());
        let want: Vec<Vec<bool>> = (1..=rounds)
            .map(|r| blocking.mask(&delay.view(r), &mut rng))
            .collect();
        assert_eq!(got, want, "variant {sched:?}");
    }
}

#[test]
fn fraction_count_is_the_largest_integer_within_the_budget() {
    for (fraction, n, expected) in [
        (0.0, 598, 0),
        (0.1, 598, 59),
        (0.25, 6, 1),
        (0.25, 8, 2),
        (0.58, 100, 58),
        (0.8333333333333333, 6, 4),
        (1.0, 6, 6),
    ] {
        let count = fraction_count(fraction, n);
        assert_eq!(count, expected, "fraction={fraction}, n={n}");
        assert!(count as f64 / n as f64 <= fraction);
        assert!(count == n || (count + 1) as f64 / n as f64 > fraction);
    }
}

#[test]
fn every_fractional_schedule_component_uses_the_hard_budget() {
    let first_round_blocked = |schedule: BlockSchedule| {
        let mut rng = ChaCha12Rng::seed_from_u64(42);
        let mut blocking = compile_blocking(&schedule, 6, &mut rng);
        let delay = SnapshotDelay::new(blocking.knowledge());
        let mask = blocking.mask(&delay.view(1), &mut rng);
        mask.iter().filter(|&&blocked| blocked).count()
    };
    let policy = crate::AdaptivePolicy::BlockHolders;
    for schedule in [
        BlockSchedule::FreshPerRound { fraction: 0.25 },
        BlockSchedule::Permanent { fraction: 0.25 },
        BlockSchedule::Adaptive0Late {
            fraction: 0.25,
            policy,
        },
        BlockSchedule::AdaptiveAlphaLate {
            fraction: 0.25,
            alpha: 1,
            policy,
        },
        BlockSchedule::Windows(vec![crate::BlockWindow {
            start_round: 1,
            rounds: 1,
            target: BlockTarget::SampleFraction(0.25),
        }]),
        BlockSchedule::PerRoundFractions(vec![0.25]),
        BlockSchedule::PerRoundSticky {
            background: 0.0,
            targets: vec![0.25],
        },
        BlockSchedule::PerRoundSticky {
            background: 0.25,
            targets: vec![0.0],
        },
    ] {
        assert_eq!(first_round_blocked(schedule.clone()), 1, "{schedule:?}");
    }
}

#[test]
fn only_the_adaptive_variants_read_node_state() {
    let policy = crate::AdaptivePolicy::BlockHolders;
    assert_eq!(
        schedule_knowledge(
            &BlockSchedule::Adaptive0Late {
                fraction: 0.1,
                policy
            },
            10
        ),
        Knowledge::NodeState { alpha: 0 }
    );
    assert_eq!(
        schedule_knowledge(
            &BlockSchedule::AdaptiveAlphaLate {
                fraction: 0.1,
                alpha: 2,
                policy
            },
            10
        ),
        Knowledge::NodeState { alpha: 2 }
    );
    assert_eq!(
        schedule_knowledge(&BlockSchedule::FreshPerRound { fraction: 0.1 }, 10),
        Knowledge::OwnHistory
    );
}

#[test]
#[should_panic(expected = "node-state schedules are drawn by the Alg-1 run loop only")]
fn history_only_drivers_refuse_node_state_strategies() {
    mask_schedule(
        &BlockSchedule::AdaptiveAlphaLate {
            fraction: 0.1,
            alpha: 1,
            policy: crate::AdaptivePolicy::BlockHolders,
        },
        10,
        3,
        &mut ChaCha12Rng::seed_from_u64(1),
    );
}

fn disjoint_masks(fraction: f64, n: usize, rounds: usize, seed: u64) -> Vec<Vec<bool>> {
    mask_schedule(
        &BlockSchedule::Disjoint { fraction },
        n,
        rounds,
        &mut ChaCha12Rng::seed_from_u64(seed),
    )
}

fn blocked_ids(mask: &[bool]) -> Vec<usize> {
    (0..mask.len()).filter(|&i| mask[i]).collect()
}

#[test]
fn disjoint_blocks_the_full_budget_outside_the_previous_round() {
    for (fraction, n) in [(0.1, 50), (0.25, 20), (0.14, 100), (0.5, 7)] {
        let b = fraction_count(fraction, n);
        for seed in [1, 2, 3] {
            let masks = disjoint_masks(fraction, n, 30, seed);
            for (t, mask) in masks.iter().enumerate() {
                assert_eq!(blocked_ids(mask).len(), b, "n={n} round {}", t + 1);
                if t > 0 {
                    assert!(
                        mask.iter()
                            .zip(&masks[t - 1])
                            .all(|(&now, &prev)| !(now && prev)),
                        "n={n} round {} re-blocks a server blocked in round {t}",
                        t + 1
                    );
                }
            }
        }
    }
}

#[test]
fn disjoint_moves_the_whole_block_every_round() {
    let masks = disjoint_masks(0.2, 30, 200, 9);
    let distinct: std::collections::BTreeSet<_> = masks.iter().map(|m| blocked_ids(m)).collect();
    assert!(
        distinct.len() > 150,
        "fresh choice must be random, not a cycle"
    );
    let mut ever = [false; 30];
    for mask in &masks {
        for i in blocked_ids(mask) {
            ever[i] = true;
        }
    }
    assert!(ever.iter().all(|&e| e), "every server must be blockable");
}

#[test]
fn disjoint_at_one_half_alternates_exact_complements() {
    let masks = disjoint_masks(0.5, 10, 12, 4);
    assert_eq!(blocked_ids(&masks[0]).len(), 5);
    for t in 1..masks.len() {
        let complement: Vec<bool> = masks[t - 1].iter().map(|&b| !b).collect();
        assert_eq!(masks[t], complement, "round {}", t + 1);
    }
}

#[test]
#[should_panic(expected = "disjoint needs fraction <= 1/2")]
fn disjoint_above_one_half_panics_at_compile() {
    disjoint_masks(0.6, 10, 2, 1);
}

#[test]
fn disjoint_precondition_is_beta_at_most_one_half() {
    assert!(disjoint_precondition(0.5).is_ok());
    assert!(disjoint_precondition(0.0).is_ok());
    let err = disjoint_precondition(0.51).unwrap_err();
    assert!(err.contains("fraction <= 1/2"), "{err}");
}

#[test]
fn disjoint_reads_only_its_own_history_and_never_stops_blocking() {
    let schedule = BlockSchedule::Disjoint { fraction: 0.1 };
    assert_eq!(schedule_knowledge(&schedule, 20), Knowledge::OwnHistory);
    let mut rng = ChaCha12Rng::seed_from_u64(0);
    assert!(!compile_blocking(&schedule, 20, &mut rng).done_blocking_after(1_000));
    let zero = BlockSchedule::Disjoint { fraction: 0.0 };
    assert!(compile_blocking(&zero, 20, &mut rng).done_blocking_after(1));
}

#[test]
fn every_noise_controller_table_parses_and_matches_its_key() {
    for (n, b) in noise_table::tabulated() {
        let table = noise_table::lookup(n, b).expect("registered table");
        assert_eq!(table.b(), b, "n={n}");
    }
    let table = noise_table::lookup(50, 5).unwrap();
    assert_eq!(table.action(0, 50, 15), (5, 0), "listed run start");
    assert_eq!(table.action(0, 50, 20), (5, 0), "listed run end");
    assert_eq!(
        table.action(0, 50, 21),
        (0, 0),
        "unlisted states block nobody"
    );
    assert_eq!(
        table.action(0, 50, 25),
        (0, 0),
        "the 50/50 start blocks nobody"
    );
    assert_eq!(
        noise_table::lookup(100, 10).unwrap().action(1, 49, 21),
        (1, 1)
    );
    assert!(noise_table::lookup(40, 4).is_none());
}

#[test]
fn noise_controller_table_parser_rejects_inconsistent_rows() {
    let header = "# format: noise-controller-table v1\n# n: 10\n# b: 2\n";
    assert!(noise_table::parse(&format!("{header}0 5 1 2 2 0\n")).is_ok());
    for bad in [
        "0 5 1 2 2 1\n",
        "0 5 1 2 0 1\n",
        "3 5 1 2 1 0\n",
        "0 11 1 2 1 0\n",
        "0 5 0 2 1 0\n",
        "0 5 3 2 1 0\n",
        "0 5 1 5 1 0\n",
        "0 5 1 2 1\n",
    ] {
        assert!(
            noise_table::parse(&format!("{header}{bad}")).is_err(),
            "{bad:?}"
        );
    }
    assert!(noise_table::parse("# format: v0\n# n: 10\n# b: 2\n").is_err());
}

fn noise_controller(n: usize, fraction: f64) -> Box<dyn BlockingStrategy> {
    compile_blocking(
        &BlockSchedule::NoiseController { fraction },
        n,
        &mut ChaCha12Rng::seed_from_u64(0),
    )
}

#[test]
fn noise_controller_is_one_late_and_acts_on_the_delayed_useful_counts() {
    let mut strategy = noise_controller(50, 0.1);
    assert_eq!(strategy.knowledge(), Knowledge::NodeState { alpha: 1 });
    let mut delay = SnapshotDelay::new(strategy.knowledge());
    let mut rng = ChaCha12Rng::seed_from_u64(5);
    let first = strategy.mask(&delay.view(1), &mut rng);
    assert_eq!(
        blocked_ids(&first).len(),
        0,
        "start state (50, 25) blocks nobody"
    );
    delay.record_useful(|| vec![(0, 33), (1, 17)]);
    delay.record(Vec::new);
    let second = strategy.mask(&delay.view(2), &mut rng);
    assert_eq!(
        blocked_ids(&second).len(),
        5,
        "U_1 = (50, a1 = 17) is in the lens"
    );
    delay.record_useful(|| vec![(0, 17), (1, 33)]);
    delay.record(Vec::new);
    let third = strategy.mask(&delay.view(3), &mut rng);
    assert_eq!(
        blocked_ids(&third).len(),
        5,
        "a1 = 33 is in the mirrored lens"
    );
    assert!(
        third
            .iter()
            .zip(&second)
            .all(|(&now, &prev)| !(now && prev))
    );
}

#[test]
fn noise_controller_reblocks_from_its_previous_mask() {
    let mut strategy = noise_controller(100, 0.1);
    let mut delay = SnapshotDelay::new(strategy.knowledge());
    let mut rng = ChaCha12Rng::seed_from_u64(8);
    strategy.mask(&delay.view(1), &mut rng);
    delay.record_useful(|| vec![(0, 26), (1, 17)]);
    let one = strategy.mask(&delay.view(2), &mut rng);
    assert_eq!(blocked_ids(&one).len(), 1, "(43, 17) at s = 0 is (1, 0)");
    delay.record_useful(|| vec![(0, 28), (1, 21)]);
    let next = strategy.mask(&delay.view(3), &mut rng);
    let ids = blocked_ids(&next);
    assert_eq!(ids.len(), 2, "(49, 21) at s = 1 is (1, 1)");
    assert!(
        ids.contains(&blocked_ids(&one)[0]),
        "the re-block is the previous server"
    );
}

#[test]
#[should_panic(expected = "two-value only")]
fn noise_controller_refuses_more_than_two_values() {
    let mut strategy = noise_controller(50, 0.1);
    let mut delay = SnapshotDelay::new(strategy.knowledge());
    let mut rng = ChaCha12Rng::seed_from_u64(5);
    strategy.mask(&delay.view(1), &mut rng);
    delay.record_useful(|| vec![(0, 10), (1, 10), (2, 10)]);
    strategy.mask(&delay.view(2), &mut rng);
}

#[test]
#[should_panic(expected = "no noise_controller table for n=40, b=4")]
fn noise_controller_without_a_table_panics_at_compile() {
    noise_controller(40, 0.1);
}

#[test]
fn noise_controller_preconditions_report_missing_tables() {
    let schedule = BlockSchedule::NoiseController { fraction: 0.1 };
    assert!(check_schedule(&schedule, 50).is_ok());
    let err = check_schedule(&schedule, 40).unwrap_err();
    assert!(
        err.contains("no noise_controller table for n=40, b=4"),
        "{err}"
    );
}

struct OverBlocking {
    declared: usize,
    mask: Vec<bool>,
}

impl BlockingStrategy for OverBlocking {
    fn knowledge(&self) -> Knowledge {
        Knowledge::OwnHistory
    }

    fn max_blocked(&self) -> usize {
        self.declared
    }

    fn mask(&mut self, _view: &View<'_>, _rng: &mut ChaCha12Rng) -> Vec<bool> {
        self.mask.clone()
    }

    fn done_blocking_after(&self, _round: usize) -> bool {
        false
    }
}

#[test]
#[should_panic(expected = "blocked 3 of 4 servers, over its declared budget of 2")]
fn a_strategy_blocking_over_its_declared_budget_is_caught() {
    let mut strategy = OverBlocking {
        declared: 2,
        mask: vec![true, true, true, false],
    };
    checked_mask(
        &mut strategy,
        &history_only_view(1),
        &mut ChaCha12Rng::seed_from_u64(0),
        4,
    );
}

#[test]
#[should_panic(expected = "mask of length 3 for n = 4")]
fn a_strategy_returning_a_wrong_length_mask_is_caught() {
    let mut strategy = OverBlocking {
        declared: 4,
        mask: vec![false; 3],
    };
    checked_mask(
        &mut strategy,
        &history_only_view(1),
        &mut ChaCha12Rng::seed_from_u64(0),
        4,
    );
}

#[test]
fn declared_budgets_are_the_true_maxima_of_the_extensible_modes() {
    let n = 20;
    let overlapping = BlockSchedule::Windows(vec![
        crate::BlockWindow {
            start_round: 1,
            rounds: 5,
            target: BlockTarget::Nodes(vec![0, 1, 2]),
        },
        crate::BlockWindow {
            start_round: 4,
            rounds: 5,
            target: BlockTarget::Nodes(vec![2, 3]),
        },
        crate::BlockWindow {
            start_round: 20,
            rounds: 1,
            target: BlockTarget::Nodes(vec![5, 6, 7]),
        },
    ]);
    let sticky = BlockSchedule::PerRoundSticky {
        background: 0.25,
        targets: vec![0.1, 0.5, 0.2],
    };
    let mut rng = ChaCha12Rng::seed_from_u64(1);
    assert_eq!(compile_blocking(&overlapping, n, &mut rng).max_blocked(), 4);
    assert_eq!(compile_blocking(&sticky, n, &mut rng).max_blocked(), 10 + 5);
}

#[test]
fn ceil_fraction_count_is_exact_for_decimal_fractions() {
    assert_eq!(ceil_fraction_count(0.55, 100), 55);
    assert_eq!(ceil_fraction_count(0.07, 100), 7);
    assert_eq!(ceil_fraction_count(0.14, 50), 7);
    assert_eq!(ceil_fraction_count(0.555, 100), 56);
    assert_eq!(ceil_fraction_count(5.0 / 9.0, 1000), 556);
    assert_eq!(ceil_fraction_count(0.0, 100), 0);
    assert_eq!(ceil_fraction_count(1.0, 100), 100);
    assert_eq!(ceil_fraction_count(0.5, 0), 0);
    assert_eq!(ceil_fraction_count(0.001, 100), 1);
}

#[test]
fn ceil_fraction_count_keeps_the_threshold_driver_counts() {
    for (fraction, n, want) in [
        (0.30, 1000, 300),
        (0.30, 4000, 1200),
        (5.0 / 9.0, 1000, 556),
        (5.0 / 9.0, 4000, 2223),
        (0.56, 1000, 560),
        (0.56, 4000, 2240),
        (0.6, 1000, 600),
        (0.7, 4000, 2800),
    ] {
        assert_eq!(ceil_fraction_count(fraction, n), want, "{fraction} x {n}");
    }
}
