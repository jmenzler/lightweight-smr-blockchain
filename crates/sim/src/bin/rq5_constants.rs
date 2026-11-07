//! SMR constants calibration runner (T_B, G, T_M, sigma intercept); run from the repo root.

use rayon::prelude::*;
use sim::analysis::{geometric_mean_growth, growth_window, ols_fit};
use sim::harness::{concurrent_injections, fresh_extended, injection};
use sim::runlog::{ledger_target, log_smr_run_to};
use sim::smr::{CommandStatus, SmrReport, SmrScenario, run_smr};

const SEEDS: u64 = 20;
const C_WIN: f64 = 1.0;
const NS: [usize; 5] = [250, 1000, 4000, 16000, 64000];
const CONCURRENT: u32 = 8;
const CSV_PATH: &str = "experiments/rq5-constants/evidence/rq5_smr.csv";
const CSV_HEADER: &str =
    "subexp,beta,sigma,n,seed,status,delivered,all_logs,prefix_fixed,tb,tm,g,last_holders";

fn opt(v: Option<usize>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}

fn run_cell(scenarios: &[SmrScenario], extra: &serde_json::Value) -> Vec<SmrReport> {
    let reports: Vec<SmrReport> = scenarios.par_iter().map(run_smr).collect();
    if let Some(ledger) = ledger_target(None) {
        for (sc, report) in scenarios.iter().zip(&reports) {
            log_smr_run_to(&ledger, sc, report, Some(extra.clone()))
                .expect("run ledger write failed (set SIM_RUNLOG=off to opt out)");
        }
    }
    reports
}

fn main() {
    let mut csv = String::from(CSV_HEADER);
    csv.push('\n');

    let arms: Vec<(&str, f64, Vec<f64>)> =
        vec![("tb", 1.0, vec![0.0, 0.1]), ("sigma5", 5.0, vec![0.0])];
    for (subexp, sigma, betas) in &arms {
        let sub_idx = if *subexp == "tb" { 0u64 } else { 2 };
        for (arm_idx, beta) in betas.iter().enumerate() {
            let mut cells: Vec<(usize, f64)> = Vec::new();
            for (n_idx, n) in NS.into_iter().enumerate() {
                let scenarios: Vec<SmrScenario> = (0..SEEDS)
                    .map(|i| {
                        let seed = 960_000
                            + 10_000 * sub_idx
                            + 1_000 * arm_idx as u64
                            + 100 * n_idx as u64
                            + i;
                        fresh_extended(n, seed, *sigma, *beta, vec![injection(3, 1, 7)], 150)
                    })
                    .collect();
                let extra = serde_json::json!(
                    { "exp": "RQ5", "subexp": subexp, "arm": beta, "sigma": sigma, "n": n }
                );
                let reports = run_cell(&scenarios, &extra);

                let mut tbs: Vec<f64> = Vec::new();
                for (sc, report) in scenarios.iter().zip(&reports) {
                    let cmd = &report.commands[0];
                    let tb = cmd
                        .all_logs_round
                        .zip(cmd.delivered_round)
                        .map(|(a, d)| a - d);
                    if let Some(t) = tb {
                        tbs.push(t as f64);
                    }
                    let g = growth_window(cmd.spread.points(), C_WIN, n)
                        .and_then(|w| geometric_mean_growth(cmd.spread.points(), w));
                    let last_holders = cmd
                        .spread
                        .points()
                        .last()
                        .map(|p| p.useful_holders)
                        .unwrap_or(0);
                    csv.push_str(&format!(
                        "{subexp},{beta},{sigma},{n},{},{:?},{},{},{},{},,{},{last_holders}\n",
                        sc.seed,
                        cmd.status,
                        opt(cmd.delivered_round),
                        opt(cmd.all_logs_round),
                        opt(cmd.prefix_fixed_round),
                        tb.map(|t| t.to_string()).unwrap_or_default(),
                        g.map(|g| format!("{g:.4}")).unwrap_or_default(),
                    ));
                }
                let mean_tb = tbs.iter().sum::<f64>() / tbs.len().max(1) as f64;
                println!(
                    "{subexp} beta={beta} n={n}: T_B complete {}/{SEEDS}, mean T_B {mean_tb:.2}",
                    tbs.len()
                );
                cells.push((n, mean_tb));
            }
            let xs: Vec<f64> = cells.iter().map(|&(n, _)| (n as f64).log2()).collect();
            let ys: Vec<f64> = cells.iter().map(|&(_, tb)| tb).collect();
            let (a, b, r2) = ols_fit(&xs, &ys).expect("fit");
            println!("{subexp} beta={beta}: T_B ~ {a:.3}*log2(n) + {b:.3}, R^2 = {r2:.4}");
        }
    }

    for (arm_idx, beta) in [0.0, 0.1].into_iter().enumerate() {
        let mut cells: Vec<(usize, f64)> = Vec::new();
        for (n_idx, n) in NS.into_iter().enumerate() {
            let scenarios: Vec<SmrScenario> = (0..SEEDS)
                .map(|i| {
                    let seed = 960_000 + 10_000 + 1_000 * arm_idx as u64 + 100 * n_idx as u64 + i;
                    fresh_extended(
                        n,
                        seed,
                        1.0,
                        beta,
                        concurrent_injections(3, CONCURRENT, 100),
                        250,
                    )
                })
                .collect();
            let extra = serde_json::json!(
                { "exp": "RQ5", "subexp": "tm", "arm": beta, "sigma": 1.0, "n": n }
            );
            let reports = run_cell(&scenarios, &extra);

            let mut tms: Vec<f64> = Vec::new();
            let mut incomplete = 0usize;
            for (sc, report) in scenarios.iter().zip(&reports) {
                for cmd in &report.commands {
                    let tm = cmd
                        .prefix_fixed_round
                        .zip(cmd.all_logs_round)
                        .map(|(p, a)| p - a);
                    if cmd.status != CommandStatus::Complete {
                        incomplete += 1;
                    } else if let Some(t) = tm {
                        tms.push(t as f64);
                    }
                    let last_holders = cmd
                        .spread
                        .points()
                        .last()
                        .map(|p| p.useful_holders)
                        .unwrap_or(0);
                    csv.push_str(&format!(
                        "tm,{beta},1,{n},{},{:?},{},{},{},,{},,{last_holders}\n",
                        sc.seed,
                        cmd.status,
                        opt(cmd.delivered_round),
                        opt(cmd.all_logs_round),
                        opt(cmd.prefix_fixed_round),
                        tm.map(|t| t.to_string()).unwrap_or_default(),
                    ));
                }
            }
            let mean_tm = tms.iter().sum::<f64>() / tms.len().max(1) as f64;
            println!(
                "tm beta={beta} n={n}: complete {}/{}, mean T_M {mean_tm:.2}",
                tms.len(),
                tms.len() + incomplete
            );
            cells.push((n, mean_tm));
        }
        let xs: Vec<f64> = cells.iter().map(|&(n, _)| (n as f64).log2()).collect();
        let ys: Vec<f64> = cells.iter().map(|&(_, tm)| tm).collect();
        let (a, b, r2) = ols_fit(&xs, &ys).expect("fit");
        println!("tm beta={beta}: T_M ~ {a:.3}*log2(n) + {b:.3}, R^2 = {r2:.4}");
    }

    std::fs::create_dir_all("experiments/rq5-constants/evidence").expect("evidence dir");
    std::fs::write(CSV_PATH, csv).expect("write csv");
    println!("per-seed rows -> {CSV_PATH}");
}
