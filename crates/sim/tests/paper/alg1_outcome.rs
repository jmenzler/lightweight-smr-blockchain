//! Alg-1 outcome: Def 1.2 agreement and availability are reported separately and state what holds at the horizon.

use sim::spec::{InitSpec, ScenarioSpec, ScheduleSpec, TargetSpec, WindowSpec};
use sim::sweep::{GridSpec, LadderRung, grid_summary, run_grid_streaming};
use sim::{
    Availability, BlockSchedule, Config, Init, Outcome, Scenario, run, run_report, run_traced,
};

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

fn holders_of(states: &[Option<u64>]) -> usize {
    states.iter().filter(|s| s.is_some()).count()
}

fn holders_unanimous_at_some_round(trace: &sim::Trace) -> bool {
    trace.rounds.iter().any(|r| {
        let mut vals = r.states.iter().flatten();
        match vals.next() {
            Some(first) => vals.all(|v| v == first),
            None => false,
        }
    })
}

// Def 1.2, p. 6, and p. 19: agreement and availability are separate; all-⊥ is agreement on ⊥.
#[test]
fn agreement_before_extinction_is_not_reported_as_died() {
    let s = scenario(
        1000,
        500,
        Init::Split { fraction: 0.5 },
        BlockSchedule::FreshPerRound { fraction: 0.15 },
        2000,
    );
    let (out, trace) = run_traced(&s);
    assert!(
        holders_unanimous_at_some_round(&trace),
        "precondition: holders became unanimous"
    );
    assert!(
        matches!(out, Outcome::Agreement { .. }),
        "holders reached unanimity before extinction (Def 1.2 agreement) but outcome is {out:?}"
    );
}

// The class must not depend on the horizon alone when the horizon only moves past the latch.
#[test]
fn outcome_class_is_independent_of_horizon() {
    let mk = |max_rounds| {
        run(&scenario(
            1000,
            49,
            Init::Split { fraction: 0.5 },
            BlockSchedule::FreshPerRound { fraction: 0.30 },
            max_rounds,
        ))
    };
    let (short, long) = (mk(5), mk(400));
    assert_eq!(
        std::mem::discriminant(&short),
        std::mem::discriminant(&long),
        "same seed/config classified {short:?} at max_rounds=5 but {long:?} at max_rounds=400"
    );
}

fn window(start_round: usize, rounds: usize, ids: Vec<u32>) -> WindowSpec {
    WindowSpec {
        start_round,
        rounds,
        target: TargetSpec::Nodes { ids },
    }
}

fn alg1_grid(base: ScenarioSpec, n: usize, seed_base: u64, seed_count: u64) -> GridSpec {
    GridSpec {
        exp: "alg1-outcome".into(),
        base,
        pairs: None,
        betas: None,
        useful_fractions: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n,
            seed_base,
            seed_count,
        }],
    }
}

fn run_alg1_grid(spec: &GridSpec, tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("alg1-outcome-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let csv = run_grid_streaming(spec, Some(&ledger), None).expect("grid runs");
    std::fs::remove_dir_all(&dir).ok();
    csv
}

// Def 1.2, p. 6: a 'survived' run needs availability, not just vacuous agreement.
#[test]
fn grid_does_not_count_a_collapsing_run_at_the_horizon_as_survived() {
    let (n, seed) = (40, 11);
    let base = ScenarioSpec {
        n,
        seed,
        k: 6,
        ell: 3,
        init: InitSpec::Split { fraction: 1.0 },
        max_rounds: 2,
        schedule: Some(ScheduleSpec::Windows {
            windows: vec![window(2, 1, (0..25).collect())],
        }),
        partition: None,
    };
    let (_, trace) = run_traced(&Scenario::try_from(base.clone()).unwrap());
    let final_holders = holders_of(&trace.rounds.last().unwrap().states);
    assert!(
        final_holders > 0 && final_holders * 4 < n,
        "setup: horizon must cut a collapsing run ({final_holders}/{n} holders)"
    );
    let csv = run_alg1_grid(&alg1_grid(base, n, seed, 1), "horizon-mech");
    let summary = grid_summary(&csv);
    assert!(
        csv.lines().nth(1).unwrap().ends_with(",collapsing,2"),
        "a run cut off below a third of the holders is labelled collapsing at the horizon: {csv}"
    );
    assert!(
        !summary[0].ends_with("1/1 survived"),
        "run ends at the horizon with {final_holders}/{n} holders, yet the grid row is '{}' and the summary reads '{}'",
        csv.lines().nth(1).unwrap(),
        summary[0]
    );
}

// Survival counts must not include horizon-censored collapses.
#[test]
fn grid_survival_count_excludes_horizon_censored_collapses_at_beta_014() {
    let (n, seed_base, seeds, horizon) = (200, 50000u64, 300u64, 300);
    let base = ScenarioSpec {
        n,
        seed: 0,
        k: 6,
        ell: 3,
        init: InitSpec::Split { fraction: 0.5 },
        max_rounds: horizon,
        schedule: Some(ScheduleSpec::FreshPerRound { fraction: 0.14 }),
        partition: None,
    };
    let csv = run_alg1_grid(
        &alg1_grid(base.clone(), n, seed_base, seeds),
        "horizon-scen",
    );
    let mut collapsing = Vec::new();
    for (i, line) in csv.lines().skip(1).enumerate() {
        if !line.contains(",agreement,") {
            continue;
        }
        let mut point = base.clone();
        point.seed = seed_base + i as u64;
        let report = run_report(&Scenario::try_from(point).unwrap());
        let last = report.metrics.last().unwrap();
        if (last.holders as usize) * 4 < n {
            collapsing.push((seed_base + i as u64, last.holders));
        }
    }
    assert!(
        collapsing.is_empty(),
        "rows labelled 'agreement' include (seed, final holders) {collapsing:?}, i.e. runs below n/4 holders mid death-spiral at the horizon"
    );
}

// A run that was unanimous and later went extinct keeps its agreement round and reports the extinction round.
#[test]
fn agreement_then_extinction_keeps_both_rounds() {
    let s = scenario(
        1000,
        49,
        Init::Split { fraction: 0.5 },
        BlockSchedule::FreshPerRound { fraction: 0.30 },
        400,
    );
    let out = run(&s);
    assert_eq!(
        out,
        Outcome::Agreement {
            value: 0,
            rounds: 5,
            availability: Availability::Extinct { at: 6 }
        }
    );
    assert_eq!(out.label(), ("died", 6));
    assert!(!out.survived());
}

// The horizon alone moves a run from agreement-with-availability to collapsing, never back to survival.
#[test]
fn horizon_inside_the_collapse_reports_collapsing_not_survival() {
    let s = scenario(
        1000,
        49,
        Init::Split { fraction: 0.5 },
        BlockSchedule::FreshPerRound { fraction: 0.30 },
        5,
    );
    assert_eq!(
        run(&s),
        Outcome::Agreement {
            value: 0,
            rounds: 5,
            availability: Availability::Collapsing { at: 5 }
        }
    );
}
