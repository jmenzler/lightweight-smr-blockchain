//! Acceptance test for `BlockSchedule::Disjoint` (exploratory): Monte Carlo over the real Alg-1
//! driver against the exact DP of the optimal-adversary work.
//!
//! DP side: `thesis/supervisor/optimal-adversary/solvers/disjoint_extinction.py`, which calls
//! `dp_scaled.solve(rule=DISJOINT)` from commit 82e598e:
//! (dp_scaled.py must sit beside the script: run on a tree containing 82e598e, or `git show` it there.)
//!   uv run --project analysis --with numba python \
//!     thesis/supervisor/optimal-adversary/solvers/disjoint_extinction.py \
//!     50:5,6,7,8 100:10,13,14,15
//! Event: |U_t| = 0 for some t ≤ H, from the full start (every server holds a value, nobody
//! blocked before round 1). Here U_t is `RoundMetrics::useful`; an all-⊥ end of round r with
//! U_r > 0 means U_{r+1} = 0.
//! Pass criterion: the DP value lies in the Wilson interval at z = 3.29 (99.9 % per cell,
//! ≥ 97.6 % jointly over the 24 cells by Bonferroni). Seeds are fixed, so the verdict is too.

use rayon::prelude::*;
use sim::{BlockSchedule, Config, Init, Scenario, run_report};

const SEEDS: u64 = 20_000;
const SEED_BASE: u64 = 7_000_000;
const HORIZONS: [usize; 3] = [10, 20, 40];
const Z: f64 = 3.29;

/// (n, fraction, b, P[extinct by 10], by 20, by 40) from the DP.
const DP: [(usize, f64, usize, [f64; 3]); 8] = [
    (
        50,
        0.10,
        5,
        [
            1.6233584486158396e-09,
            5.983034583555762e-07,
            2.75498103807134e-06,
        ],
    ),
    (
        50,
        0.12,
        6,
        [
            3.044439707465676e-05,
            0.006512613875345714,
            0.031864050568042604,
        ],
    ),
    (
        50,
        0.14,
        7,
        [0.014896020434714836, 0.5007930908660015, 0.9268573413888305],
    ),
    (
        50,
        0.16,
        8,
        [0.3418506126961778, 0.9944437790075823, 0.9999998644197748],
    ),
    (
        100,
        0.10,
        10,
        [
            1.4499764444289672e-17,
            9.936651302689662e-13,
            6.8528620820012635e-12,
        ],
    ),
    (
        100,
        0.13,
        13,
        [
            3.8386364363738325e-06,
            0.024487599071086896,
            0.18107008698513694,
        ],
    ),
    (
        100,
        0.14,
        14,
        [0.0006821953973028151, 0.4214810674088418, 0.940296145718898],
    ),
    (
        100,
        0.15,
        15,
        [0.025157607390220965, 0.9443950931272681, 0.9999858251207977],
    ),
];

/// First round t with |U_t| = 0, if one exists within the run or right after its all-⊥ end.
fn first_extinct_round(n: usize, fraction: f64, b: usize, seed: u64) -> Option<usize> {
    let report = run_report(&Scenario {
        n,
        seed,
        cfg: Config::default(),
        init: Init::Split { fraction: 0.5 },
        max_rounds: HORIZONS[2],
        schedule: BlockSchedule::Disjoint { fraction },
        partition: None,
    });
    assert_eq!(report.metrics[0].blocked as usize, b, "budget for n={n}");
    assert_eq!(report.metrics[0].useful as usize, n - b, "full start");
    if let Some(i) = report.metrics.iter().position(|m| m.useful == 0) {
        return Some(i + 1);
    }
    report.outcome.extinction_round().map(|rounds| rounds + 1)
}

fn wilson(k: u64, total: u64) -> (f64, f64, f64) {
    let (k, total) = (k as f64, total as f64);
    let p = k / total;
    let z2 = Z * Z;
    let denom = 1.0 + z2 / total;
    let center = (p + z2 / (2.0 * total)) / denom;
    let half = Z / denom * (p * (1.0 - p) / total + z2 / (4.0 * total * total)).sqrt();
    (p, center - half, center + half)
}

#[test]
fn disjoint_extinction_matches_the_exact_dp() {
    let mut failures = Vec::new();
    eprintln!("n,b,H,mc,wilson_lo,wilson_hi,dp");
    for (n, fraction, b, dp) in DP {
        let firsts: Vec<Option<usize>> = (SEED_BASE..SEED_BASE + SEEDS)
            .into_par_iter()
            .map(|seed| first_extinct_round(n, fraction, b, seed))
            .collect();
        for (h, want) in HORIZONS.iter().zip(dp) {
            let k = firsts.iter().filter(|f| f.is_some_and(|t| t <= *h)).count() as u64;
            let (p, lo, hi) = wilson(k, SEEDS);
            eprintln!("{n},{b},{h},{p:.5},{lo:.5},{hi:.5},{want:.5}");
            if !(lo..=hi).contains(&want) {
                failures.push(format!(
                    "n={n} b={b} H={h}: MC {p:.5} [{lo:.5}, {hi:.5}] vs DP {want:.5}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
