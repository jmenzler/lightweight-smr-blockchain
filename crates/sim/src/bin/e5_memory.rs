//! Uncommitted-pool flatness runner under sustained load (Thm 5's L_i term); run from the repo root.
//! It gates max |L_i| only; the shared-state size and the per-client sn map are not measured.

use rayon::prelude::*;
use sim::analysis::ols_fit;
use sim::runlog::{ledger_target, record_smr_report};
use sim::smr::{
    ClientModel, CommandStatus, Proto, SmrReport, SmrScenario, TrafficPhase, point_mass_pmf,
    run_smr, run_smr_logged,
};
use sim::{BlockSchedule, Config};

// Each concurrent n=1024 run holds ~1.5 GB; lower E5_PARALLEL on RAM-tight hosts.
fn max_parallel() -> usize {
    std::env::var("E5_PARALLEL")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&p| p >= 1)
        .unwrap_or(4)
}

const SEEDS: u64 = 10;
const SIGMA: f64 = 1.0;
const NS: [usize; 3] = [64, 256, 1024];
const RATE: usize = 4;
const LOAD_ROUNDS: usize = 2000;
const DRAIN_ROUNDS: usize = 500;
const STEADY_FROM: usize = 500;
const EXT_ROUNDS: usize = 300;
const EXT_STEADY_FROM: usize = 100;
const FLAT_SLOPE_MAX: f64 = 0.02;
const GROWTH_SLOPE_MIN: f64 = 0.5 * RATE as f64;

// Clears the measured rate-4 violation envelope (T* ≤ 46 at n ≤ 1024) by ≥ 22 rounds.
fn t_commit(n: usize) -> u64 {
    (10.0 * (n as f64).log2()).ceil() as u64
}

fn log_len_slope(metrics: &[sim::smr::SmrRoundMetrics], from: usize, to: usize) -> f64 {
    let xs: Vec<f64> = (from..=to).map(|r| r as f64).collect();
    let ys: Vec<f64> = metrics[from - 1..to]
        .iter()
        .map(|m| m.max_log_len as f64)
        .collect();
    ols_fit(&xs, &ys).expect("steady window fits").0
}

fn main() {
    let max_parallel = max_parallel();
    let mut failures: Vec<String> = Vec::new();

    for (arm_idx, beta) in [0.0, 0.1].into_iter().enumerate() {
        for (n_idx, n) in NS.into_iter().enumerate() {
            let t = t_commit(n);
            let deadline = LOAD_ROUNDS + t as usize + 4 * (n as f64).log2().ceil() as usize + 25;
            let scenarios: Vec<SmrScenario> = (0..SEEDS)
                .map(|i| SmrScenario {
                    n,
                    seed: 950_000 + 10_000 * arm_idx as u64 + 1_000 * n_idx as u64 + i,
                    cfg: Config::default(),
                    sigma: SIGMA,
                    proto: Proto::Compact { t_commit_rounds: t },
                    injections: vec![],
                    max_rounds: LOAD_ROUNDS + DRAIN_ROUNDS,
                    schedule: BlockSchedule::FreshPerRound { fraction: beta },
                    traffic: Some(vec![
                        TrafficPhase {
                            from_round: 1,
                            arrivals_pmf: point_mass_pmf(RATE),
                        },
                        TrafficPhase {
                            from_round: LOAD_ROUNDS + 1,
                            arrivals_pmf: vec![1.0],
                        },
                    ]),
                    client_model: ClientModel::Unique,
                    certs: false,
                    manual_blocks: Vec::new(),
                    partition: None,
                    halt_on_violation: false,
                    merge_policy: Default::default(),
                    repeated_commit: Default::default(),
                })
                .collect();
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(max_parallel)
                .build()
                .expect("thread pool");
            let reports: Vec<SmrReport> =
                pool.install(|| scenarios.par_iter().map(run_smr).collect());
            let ledger = ledger_target(None);

            let mut slopes: Vec<f64> = Vec::new();
            let mut tails: Vec<f64> = Vec::new();
            let mut drained = 0usize;
            for (i, (scenario, report)) in scenarios.iter().zip(&reports).enumerate() {
                if let Some(ledger) = &ledger {
                    let extra = serde_json::json!({
                        "exp": "E5", "proto": "compact", "arm": beta, "n": n, "T": t
                    });
                    record_smr_report(scenario, report, ledger, Some(extra));
                }
                assert!(
                    report.safety_ok,
                    "split brain at beta={beta} n={n} seed {i}"
                );
                slopes.push(log_len_slope(&report.metrics, STEADY_FROM, LOAD_ROUNDS));
                let steady_median = {
                    let mut ls: Vec<u32> = report.metrics[STEADY_FROM - 1..LOAD_ROUNDS]
                        .iter()
                        .map(|m| m.max_log_len)
                        .collect();
                    ls.sort_unstable();
                    ls[ls.len() / 2] as f64
                };
                tails.push(steady_median);
                let all_complete = report
                    .commands
                    .iter()
                    .all(|c| c.status == CommandStatus::Complete);
                let last_ack = report
                    .commands
                    .iter()
                    .filter_map(|c| c.committed_ack_round)
                    .max()
                    .unwrap_or(0);
                if all_complete && last_ack <= deadline {
                    drained += 1;
                }
            }
            slopes.sort_by(f64::total_cmp);
            let median_slope = slopes[slopes.len() / 2];
            tails.sort_by(f64::total_cmp);
            let median_tail = tails[tails.len() / 2];
            let cell = format!("compact beta={beta} n={n}");
            println!(
                "{cell}: T={t}, slope median {median_slope:+.4} /round, steady max|L| \
                 median {median_tail:.0} (rate*T = {}), drained {drained}/{SEEDS} \
                 (deadline r{deadline})",
                RATE as u64 * t
            );
            if median_slope.abs() > FLAT_SLOPE_MAX {
                failures.push(format!("{cell} slope"));
            }
            if drained != SEEDS as usize {
                failures.push(format!("{cell} drain"));
            }
        }
    }

    for (n_idx, n) in NS.into_iter().enumerate() {
        let mut slopes: Vec<f64> = Vec::new();
        for i in 0..SEEDS {
            let scenario = SmrScenario {
                n,
                seed: 960_000 + 1_000 * n_idx as u64 + i,
                cfg: Config::default(),
                sigma: SIGMA,
                proto: Proto::Extended,
                injections: vec![],
                max_rounds: EXT_ROUNDS,
                schedule: BlockSchedule::FreshPerRound { fraction: 0.0 },
                traffic: Some(vec![TrafficPhase {
                    from_round: 1,
                    arrivals_pmf: point_mass_pmf(RATE),
                }]),
                client_model: ClientModel::Unique,
                certs: false,
                manual_blocks: Vec::new(),
                partition: None,
                halt_on_violation: false,
                merge_policy: Default::default(),
                repeated_commit: Default::default(),
            };
            let extra = serde_json::json!({ "exp": "E5", "proto": "extended", "n": n });
            let report = run_smr_logged(&scenario, Some(extra));
            slopes.push(log_len_slope(&report.metrics, EXT_STEADY_FROM, EXT_ROUNDS));
        }
        slopes.sort_by(f64::total_cmp);
        let median_slope = slopes[slopes.len() / 2];
        let cell = format!("extended n={n}");
        println!("{cell}: log growth slope median {median_slope:+.3} /round (rate {RATE})");
        if median_slope < GROWTH_SLOPE_MIN {
            failures.push(format!("{cell} growth"));
        }
    }

    if failures.is_empty() {
        println!("E5 PASS — all cells within registered criteria (max |L_i| only)");
    } else {
        println!("E5 FAIL — outside criteria: {failures:?}");
        std::process::exit(1);
    }
}
