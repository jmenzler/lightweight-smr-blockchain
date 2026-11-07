//! Commitment / prefix-agreement runner (Thm 3 / Lemma 3.8); run from the repo root.
//! T_M here is Lemma 3.8's: prefix_fixed_round (identical prefix through x in every non-⊥ log)
//! minus all_logs_round, an upper bound on the Def 1.3 same-index agreement time.

use sim::analysis::ols_fit;
use sim::harness::{concurrent_injections, fresh_extended};
use sim::smr::{CommandStatus, run_smr_logged};

const SEEDS: u64 = 20;
const SIGMA: f64 = 1.0;
const CONCURRENT: u32 = 8;
const NS: [usize; 4] = [250, 1000, 4000, 16000];
const TM_BOUND_FACTOR: f64 = 4.0;

fn main() {
    let mut failures: Vec<String> = Vec::new();
    for (arm_idx, beta) in [0.0, 0.1].into_iter().enumerate() {
        let mut mean_tms: Vec<f64> = Vec::new();
        for (n_idx, n) in NS.into_iter().enumerate() {
            let mut tms: Vec<usize> = Vec::new();
            let mut incomplete = 0usize;
            for i in 0..SEEDS {
                let scenario = fresh_extended(
                    n,
                    940_000 + 1000 * arm_idx as u64 + 100 * n_idx as u64 + i,
                    SIGMA,
                    beta,
                    concurrent_injections(3, CONCURRENT, 100),
                    250,
                );
                let extra = serde_json::json!({ "exp": "E4", "arm": beta, "n": n });
                let report = run_smr_logged(&scenario, Some(extra));
                for cmd in &report.commands {
                    if cmd.status != CommandStatus::Complete {
                        incomplete += 1;
                        continue;
                    }
                    tms.push(
                        cmd.prefix_fixed_round.expect("complete")
                            - cmd.all_logs_round.expect("complete"),
                    );
                }
            }
            let bound = (TM_BOUND_FACTOR * (n as f64).log2()).floor() as usize;
            let max_tm = tms.iter().max().copied().unwrap_or(0);
            let mean_tm = tms.iter().sum::<usize>() as f64 / tms.len().max(1) as f64;
            mean_tms.push(mean_tm);
            let cell = format!("beta={beta} n={n}");
            println!(
                "{cell}: complete {}/{}, T_M mean {mean_tm:.2} max {max_tm} (bound {bound})",
                tms.len(),
                tms.len() + incomplete
            );
            if incomplete > 0 || max_tm > bound {
                failures.push(cell);
            }
        }
        let xs: Vec<f64> = NS.iter().map(|&n| (n as f64).log2()).collect();
        let (slope, intercept, r2) = ols_fit(&xs, &mean_tms).expect("fit");
        println!(
            "arm beta={beta} (descriptive): T_M ~ {slope:.3}*log2(n) + {intercept:.3}, R^2 = {r2:.4}"
        );
    }
    if failures.is_empty() {
        println!("E4 PASS — all cells within registered criteria");
    } else {
        println!("E4 FAIL — outside criteria: {failures:?}");
        std::process::exit(1);
    }
}
