use protocol::Config;
use sim::{BlockSchedule, Init, Outcome, Scenario, run_report, run_traced};

fn scenario(
    n: usize,
    seed: u64,
    init: Init,
    schedule: BlockSchedule,
    max_rounds: usize,
) -> Scenario {
    Scenario {
        n,
        seed,
        cfg: Config::default(),
        init,
        max_rounds,
        schedule,
        partition: None,
    }
}

#[test]
fn one_metrics_row_per_simulated_round() {
    let s = scenario(
        100,
        7,
        Init::Split { fraction: 0.5 },
        BlockSchedule::FreshPerRound { fraction: 0.1 },
        200,
    );
    let report = run_report(&s);
    let (_, trace) = run_traced(&s);
    assert_eq!(report.metrics.len(), trace.rounds.len());
}

#[test]
fn same_seed_same_metrics() {
    let s = scenario(
        150,
        3,
        Init::UniformRandom { k: 4 },
        BlockSchedule::Permanent { fraction: 0.15 },
        100,
    );
    assert_eq!(run_report(&s).metrics, run_report(&s).metrics);
}

#[test]
fn counters_match_the_trace_exactly() {
    // The trace is the ground truth: recompute every counter from it.
    // useful_r = snapshot entering round r (initial for r=1) ∧ not blocked in r;
    // holders/undecided/distinct describe the states AFTER round r's step.
    let s = scenario(
        200,
        11,
        Init::WithUndecided {
            useful_fraction: 0.6,
            inner: Box::new(Init::UniformRandom { k: 3 }),
        },
        BlockSchedule::FreshPerRound { fraction: 0.2 },
        150,
    );
    let report = run_report(&s);
    let (_, trace) = run_traced(&s);

    let mut snapshot = trace.initial.clone();
    for (row, round) in report.metrics.iter().zip(&trace.rounds) {
        let blocked: std::collections::HashSet<u32> = round.blocked.iter().copied().collect();
        let useful = snapshot
            .iter()
            .enumerate()
            .filter(|(i, s)| s.is_some() && !blocked.contains(&(*i as u32)))
            .count() as u32;
        let holders = round.states.iter().flatten().count() as u32;
        let distinct = round
            .states
            .iter()
            .flatten()
            .collect::<std::collections::HashSet<_>>()
            .len() as u32;

        assert_eq!(row.useful, useful);
        assert_eq!(row.blocked, round.blocked.len() as u32);
        assert_eq!(row.holders, holders);
        assert_eq!(row.undecided, s.n as u32 - holders);
        assert_eq!(row.distinct_values, distinct);
        snapshot = round.states.clone();
    }
}

/// P(Bin(6, p) ≥ 3): a server with six uniform requests, each answered with probability p,
/// gets at least ℓ = 3 replies. Counted from the binomial formula, not from the simulator.
fn at_least_three_of_six(p: f64) -> f64 {
    let choose = [1.0, 6.0, 15.0, 20.0, 15.0, 6.0, 1.0];
    (3..=6)
        .map(|j| choose[j] * p.powi(j as i32) * (1.0 - p).powi(6 - j as i32))
        .sum()
}

#[test]
fn one_round_useful_count_follows_the_mean_field_map() {
    // Alg 1 with k = 6, ℓ = 3 and b = ⌊βn⌋ blocked servers drawn independently each round: a
    // request is answered iff its target is useful (probability u/n, self included), an
    // unblocked server becomes a holder iff ≥ 3 of its 6 requests are answered, and a holder
    // is useful in the next round iff it is unblocked there.
    //   fresh blocking:     E[u₂ | u₁] = (n−b)²/n · P(Bin(6, u₁/n) ≥ 3)
    //   permanent blocking: E[u₂ | u₁] = (n−b)   · P(Bin(6, u₁/n) ≥ 3)
    // Mean residual over 400 seeds within 5 standard errors. A rule where blocked servers keep
    // their value, a shifted threshold, or self-exclusion moves the mean by far more.
    let n = 200usize;
    let seeds = 400;
    for fresh in [true, false] {
        for u0 in [0.35, 0.5, 0.7, 0.9] {
            let beta = if fresh { 0.1 } else { 0.2 };
            let b = (beta * n as f64).floor();
            let schedule = if fresh {
                BlockSchedule::FreshPerRound { fraction: beta }
            } else {
                BlockSchedule::Permanent { fraction: beta }
            };
            let residuals: Vec<f64> = (0..seeds)
                .map(|seed| {
                    let report = run_report(&scenario(
                        n,
                        seed,
                        Init::WithUndecided {
                            useful_fraction: u0,
                            inner: Box::new(Init::Split { fraction: 0.5 }),
                        },
                        schedule.clone(),
                        2,
                    ));
                    let (u1, u2) = (report.metrics[0].useful, report.metrics[1].useful);
                    let q = at_least_three_of_six(f64::from(u1) / n as f64);
                    let unblocked = n as f64 - b;
                    let expected = if fresh {
                        unblocked * unblocked / n as f64
                    } else {
                        unblocked
                    } * q;
                    f64::from(u2) - expected
                })
                .collect();
            let mean = residuals.iter().sum::<f64>() / seeds as f64;
            let var =
                residuals.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (seeds - 1) as f64;
            let tolerance = 5.0 * (var / seeds as f64).sqrt();
            assert!(
                mean.abs() <= tolerance,
                "fresh={fresh} u0={u0}: mean residual {mean:.3} exceeds {tolerance:.3}"
            );
        }
    }
}

#[test]
fn zero_blocking_survival_flips_at_the_unstable_fixed_point_of_the_mean_field_map() {
    // Lemma 2.1 / 2.2 without blocking: u ↦ P(Bin(6, u) ≥ 3) has an unstable fixed point u* ≈ 0.348
    // (just above the lemma's 1/3 − ε death bound and below its 1/2 survival bound). Starting
    // 0.05 below u*, every seed dies; 0.05 above, every seed keeps a live population.
    let (mut lo, mut hi) = (0.2, 0.5);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if at_least_three_of_six(mid) < mid {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let fixed_point = (lo + hi) / 2.0;
    assert!(
        (0.34..0.36).contains(&fixed_point),
        "u* = {fixed_point}, expected ≈ 0.348"
    );
    for seed in 0..12 {
        let run_from = |u0: f64| {
            run_report(&scenario(
                2000,
                seed,
                Init::WithUndecided {
                    useful_fraction: u0,
                    inner: Box::new(Init::Split { fraction: 0.5 }),
                },
                BlockSchedule::FreshPerRound { fraction: 0.0 },
                400,
            ))
            .outcome
        };
        let below = run_from(fixed_point - 0.05);
        assert!(
            matches!(below, Outcome::AllUndecided { .. }),
            "seed {seed}: u0 below u* must die, got {below:?}"
        );
        let above = run_from(fixed_point + 0.05);
        assert!(
            matches!(above, Outcome::Agreement { .. }),
            "seed {seed}: u0 above u* must survive, got {above:?}"
        );
    }
}

/// Largest blocked fraction at which a non-empty fixed point of u ↦ factor(β) · P(Bin(6, u) ≥ 3)
/// still exists, found by bisection on max over u of (factor · q(u) − u).
fn blocking_threshold(factor: impl Fn(f64) -> f64) -> f64 {
    let survives = |beta: f64| {
        (1..1000)
            .map(|i| f64::from(i) / 1000.0)
            .any(|u| factor(beta) * at_least_three_of_six(u) >= u)
    };
    let (mut lo, mut hi) = (0.0, 0.5);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if survives(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

#[test]
fn survival_flips_at_the_mean_field_blocking_thresholds() {
    // Fresh blocking: u ↦ (1−β)² q(u); permanent blocking: u ↦ (1−b) q(u). Past the largest
    // β (resp. b) with a non-empty fixed point the population dies in every seed, 0.03 below
    // it every seed survives. The thresholds come out as ≈ 0.145 and ≈ 0.270.
    let fresh = blocking_threshold(|beta| (1.0 - beta) * (1.0 - beta));
    let permanent = blocking_threshold(|b| 1.0 - b);
    assert!((0.14..0.15).contains(&fresh), "fresh β* = {fresh}");
    assert!(
        (0.26..0.28).contains(&permanent),
        "permanent b* = {permanent}"
    );
    for (name, threshold, schedule_at) in [
        (
            "fresh",
            fresh,
            (|x| BlockSchedule::FreshPerRound { fraction: x }) as fn(f64) -> BlockSchedule,
        ),
        (
            "permanent",
            permanent,
            (|x| BlockSchedule::Permanent { fraction: x }) as fn(f64) -> BlockSchedule,
        ),
    ] {
        for seed in 0..8 {
            let run_at = |x: f64| {
                run_report(&scenario(
                    2000,
                    seed,
                    Init::Split { fraction: 0.5 },
                    schedule_at(x),
                    600,
                ))
                .outcome
            };
            let below = run_at(threshold - 0.03);
            assert!(
                below.survived(),
                "{name} seed {seed}: below the threshold must survive, got {below:?}"
            );
            let above = run_at(threshold + 0.03);
            assert!(
                !above.survived(),
                "{name} seed {seed}: above the threshold must die, got {above:?}"
            );
        }
    }
}

#[test]
fn oblivious_fresh_blocking_smoke_useful_fraction_converges_to_three_quarters_and_holds() {
    // Smoke check in the regime of Lemma 2.2 (single seed, oblivious blocking, not the lemma's
    // 1-late adaptive adversary): from ≥ 1/2 useful under β = 1/10 fresh-per-round, the
    // useful fraction reaches ≥ 3/4 and never drops below it again.
    // "Useful" is the paper's sense (non-⊥ ∧ non-blocked), so the ≥ 1/2
    // precondition needs initial HOLDERS ≥ 1/(2(1−β)) = 5/9 — a holder
    // fraction of exactly 1/2 sits on the unstable fixed point and dies.
    let s = scenario(
        4000,
        42,
        Init::WithUndecided {
            useful_fraction: 0.56,
            inner: Box::new(Init::Split { fraction: 0.5 }),
        },
        BlockSchedule::FreshPerRound { fraction: 0.10 },
        300,
    );
    let report = run_report(&s);
    assert!(
        matches!(report.outcome, Outcome::Agreement { .. }),
        "got {:?}",
        report.outcome
    );
    let threshold = (0.75 * 4000.0) as u32;
    let crossing = report
        .metrics
        .iter()
        .position(|row| row.useful >= threshold)
        .expect("useful fraction never reached 3/4");
    for (i, row) in report.metrics.iter().enumerate().skip(crossing) {
        assert!(
            row.useful >= threshold,
            "useful dropped below 3/4 at round {} after crossing at {}",
            i + 1,
            crossing + 1
        );
    }
}
