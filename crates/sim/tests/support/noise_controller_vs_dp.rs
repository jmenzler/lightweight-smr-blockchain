//! Acceptance test for `BlockSchedule::NoiseController` (exploratory): Monte Carlo of the real
//! Alg-1 driver against the exact DP that produced its tables.
//!
//! Objective (dp_scaled "time_to_agree"): T = the first round t ≥ 1 whose useful set U_t holds
//! fewer than two values; the DP's value at its start is E[T] (it counts the fictitious U_0).
//! The DP start is reproduced exactly by an iid uniform two-value init: U_1's values are then
//! Bin(|U_1|, 1/2), as the DP's first transition assumes. DP values are the `start_*` header
//! lines of `crates/sim/data/noise_controller/n{n}_b{b}.txt` (dp_scaled.py at 82e598e).
//! Each seed runs the noise controller, block-nobody and Disjoint on the same seed (paired).
//! Pass criteria, all at z = 3.29 (99.9 % per interval):
//! - each strategy's mean T contains the DP value in its interval;
//! - the paired gap noise controller − Disjoint is positive (the DP gaps are 0.35–0.71 rounds);
//! - the paired gap noise controller − nobody contains the DP gap. That gap is 0.007–0.065
//!   rounds, so whether its interval also excludes 0 is reported here and asserted, at 50 000
//!   seeds for (100, 14), by the separation test below.

use rayon::prelude::*;
use sim::{BlockSchedule, Config, Init, Scenario, Value, run_traced};

const SEEDS: u64 = 20_000;
const SEED_BASE: u64 = 9_000_000;
const MAX_ROUNDS: usize = 80;
const Z: f64 = 3.29;
const TABLES: [(usize, usize); 6] = [(30, 3), (50, 5), (50, 7), (100, 10), (100, 14), (200, 20)];

fn dp_start_values(n: usize, b: usize) -> [f64; 3] {
    let path = format!(
        "{}/data/noise_controller/n{n}_b{b}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("table file");
    let field = |key: &str| -> f64 {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("# {key}: ")))
            .unwrap_or_else(|| panic!("{path} lacks {key}"))
            .parse()
            .expect("numeric header")
    };
    [
        field("start_optimal"),
        field("start_nobody"),
        field("start_disjoint"),
    ]
}

/// First round whose useful set holds fewer than two values.
fn agreement_round(n: usize, seed: u64, schedule: BlockSchedule) -> usize {
    let (_, trace) = run_traced(&Scenario {
        n,
        seed,
        cfg: Config::default(),
        init: Init::UniformRandom { k: 2 },
        max_rounds: MAX_ROUNDS,
        schedule,
        partition: None,
    });
    let mut before = trace.initial.clone();
    for (t, round) in trace.rounds.iter().enumerate() {
        let mut values: Vec<Value> = round
            .blocked
            .iter()
            .fold(before.clone(), |mut s, &i| {
                s[i as usize] = None;
                s
            })
            .into_iter()
            .flatten()
            .collect();
        values.sort_unstable();
        values.dedup();
        if values.len() < 2 {
            return t + 1;
        }
        before.clone_from(&round.states);
    }
    // A run stops early once agreed with nobody left to block, or all ⊥; U_{r+1} ⊆ the holders after round r.
    let mut holders: Vec<Value> = before.into_iter().flatten().collect();
    holders.sort_unstable();
    holders.dedup();
    if holders.len() < 2 && trace.rounds.len() < MAX_ROUNDS {
        return trace.rounds.len() + 1;
    }
    panic!("seed {seed}: still split after {MAX_ROUNDS} rounds (censored)");
}

fn mean_ci(xs: &[f64]) -> (f64, f64, f64) {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let half = Z * (var / n).sqrt();
    (mean, mean - half, mean + half)
}

#[test]
fn noise_controller_agreement_time_matches_the_exact_dp() {
    let mut failures = Vec::new();
    eprintln!("n,b,arm,mc_mean,ci_lo,ci_hi,dp");
    for (n, b) in TABLES {
        let fraction = b as f64 / n as f64;
        let [dp_opt, dp_nobody, dp_disjoint] = dp_start_values(n, b);
        let rows: Vec<[f64; 3]> = (SEED_BASE..SEED_BASE + SEEDS)
            .into_par_iter()
            .map(|seed| {
                [
                    BlockSchedule::NoiseController { fraction },
                    BlockSchedule::FreshPerRound { fraction: 0.0 },
                    BlockSchedule::Disjoint { fraction },
                ]
                .map(|schedule| agreement_round(n, seed, schedule) as f64)
            })
            .collect();
        let column = |f: &dyn Fn(&[f64; 3]) -> f64| rows.iter().map(f).collect::<Vec<f64>>();
        let arms = [
            ("noise_controller", column(&|r| r[0]), dp_opt),
            ("nobody", column(&|r| r[1]), dp_nobody),
            ("disjoint", column(&|r| r[2]), dp_disjoint),
            ("nc-nobody", column(&|r| r[0] - r[1]), dp_opt - dp_nobody),
            (
                "nc-disjoint",
                column(&|r| r[0] - r[2]),
                dp_opt - dp_disjoint,
            ),
        ];
        for (arm, xs, dp) in arms {
            let (mean, lo, hi) = mean_ci(&xs);
            eprintln!("{n},{b},{arm},{mean:.4},{lo:.4},{hi:.4},{dp:.4}");
            if !(lo..=hi).contains(&dp) {
                failures.push(format!(
                    "n={n} b={b} {arm}: MC {mean:.4} [{lo:.4}, {hi:.4}] vs DP {dp:.4}"
                ));
            }
            if arm == "nc-disjoint" && lo <= 0.0 {
                failures.push(format!(
                    "n={n} b={b}: noise controller not resolvably slower than Disjoint"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Fixed before the first run from the paired SD seen at 20 000 seeds (about 2.4 rounds for
/// (100, 14)): at N = 50 000 its DP gap 0.0647 sits about 6.1 standard errors above 0, so
/// `lo > 0` holds with power about 99.7 %. A block-nobody controller fails it (gap 0 or noise around 0), which the
/// 20 000-seed test above cannot guarantee: its gap interval is ±0.05 rounds.
const SEPARATION_SEEDS: u64 = 50_000;
const SEPARATION_SEED_BASE: u64 = 30_000_000;

#[test]
fn noise_controller_is_resolvably_slower_than_nobody_at_100_14() {
    let (n, b) = (100, 14);
    let fraction = b as f64 / n as f64;
    let [dp_opt, dp_nobody, _] = dp_start_values(n, b);
    let gaps: Vec<f64> = (SEPARATION_SEED_BASE..SEPARATION_SEED_BASE + SEPARATION_SEEDS)
        .into_par_iter()
        .map(|seed| {
            agreement_round(n, seed, BlockSchedule::NoiseController { fraction }) as f64
                - agreement_round(n, seed, BlockSchedule::FreshPerRound { fraction: 0.0 }) as f64
        })
        .collect();
    let (mean, lo, hi) = mean_ci(&gaps);
    let dp_gap = dp_opt - dp_nobody;
    eprintln!("n={n} b={b} nc-nobody {mean:.4} [{lo:.4}, {hi:.4}] dp {dp_gap:.4}");
    assert!(
        lo > 0.0,
        "noise controller not resolvably slower than nobody"
    );
    assert!(
        (lo..=hi).contains(&dp_gap),
        "gap interval misses the DP gap"
    );
}

/// Fixed before the first run: N, seed range and which cells must resolve. With the paired SD
/// of about 2.2 rounds seen at 20 000 seeds, N = 200 000 resolves DP gaps ≥ 0.036 with high
/// probability; (30, 3) (gap 0.007) and (50, 5) (gap 0.026) are reported, not asserted.
const POWER_SEEDS: u64 = 200_000;
const POWER_SEED_BASE: u64 = 20_000_000;
const POWER_ASSERTED: [(usize, usize); 4] = [(50, 7), (100, 10), (100, 14), (200, 20)];

/// cargo test --release -p sim --test support noise_controller_beats_nobody -- --ignored --nocapture
#[test]
#[ignore = "minutes of Monte Carlo; run on demand"]
fn noise_controller_beats_nobody_where_the_dp_gap_is_resolvable() {
    let mut failures = Vec::new();
    eprintln!("n,b,nc_minus_nobody,ci_lo,ci_hi,dp_gap,asserted");
    for (n, b) in TABLES {
        let fraction = b as f64 / n as f64;
        let [dp_opt, dp_nobody, _] = dp_start_values(n, b);
        let gaps: Vec<f64> = (POWER_SEED_BASE..POWER_SEED_BASE + POWER_SEEDS)
            .into_par_iter()
            .map(|seed| {
                agreement_round(n, seed, BlockSchedule::NoiseController { fraction }) as f64
                    - agreement_round(n, seed, BlockSchedule::FreshPerRound { fraction: 0.0 })
                        as f64
            })
            .collect();
        let (mean, lo, hi) = mean_ci(&gaps);
        let dp_gap = dp_opt - dp_nobody;
        let asserted = POWER_ASSERTED.contains(&(n, b));
        eprintln!("{n},{b},{mean:.4},{lo:.4},{hi:.4},{dp_gap:.4},{asserted}");
        if !(lo..=hi).contains(&dp_gap) {
            failures.push(format!("n={n} b={b}: gap interval misses the DP gap"));
        }
        if asserted && lo <= 0.0 {
            failures.push(format!("n={n} b={b}: gap not resolvably positive"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
