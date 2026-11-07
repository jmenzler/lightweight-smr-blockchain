//! Broadcast verification runner (Thm 4 / Lemma 3.1 / Cor 3.7); run from the repo root.

use sim::analysis::{geometric_mean_growth, growth_window, ols_fit};
use sim::harness::{fresh_extended, injection};
use sim::smr::run_smr_logged;

const SEEDS: u64 = 20;
const SIGMA: f64 = 1.0;
const C_WIN: f64 = 1.0;
const NS: [usize; 4] = [250, 1000, 4000, 16000];
const MIN_CONCLUSIVE: usize = 12;

fn main() {
    let mut failures: Vec<String> = Vec::new();
    for (arm_idx, beta) in [0.0, 0.1].into_iter().enumerate() {
        let mut mean_tbs: Vec<f64> = Vec::new();
        for (n_idx, n) in NS.into_iter().enumerate() {
            let mut gs: Vec<f64> = Vec::new();
            let mut tbs: Vec<f64> = Vec::new();
            let mut incomplete = 0usize;
            for i in 0..SEEDS {
                let scenario = fresh_extended(
                    n,
                    930_000 + 1000 * arm_idx as u64 + 100 * n_idx as u64 + i,
                    SIGMA,
                    beta,
                    vec![injection(3, 1, 7)],
                    150,
                );
                let extra = serde_json::json!({ "exp": "E3", "arm": beta, "n": n });
                let report = run_smr_logged(&scenario, Some(extra));
                let cmd = &report.commands[0];
                let (Some(delivered), Some(all_logs)) = (cmd.delivered_round, cmd.all_logs_round)
                else {
                    incomplete += 1;
                    continue;
                };
                tbs.push((all_logs - delivered) as f64);
                if let Some(g) = growth_window(cmd.spread.points(), C_WIN, n)
                    .and_then(|w| geometric_mean_growth(cmd.spread.points(), w))
                {
                    gs.push(g);
                }
            }
            gs.sort_by(f64::total_cmp);
            let conclusive = gs.len();
            let median = gs.get(conclusive / 2).copied();
            let above = gs.iter().filter(|&&g| g >= 1.5).count();
            let mean_tb = if tbs.is_empty() {
                f64::NAN
            } else {
                tbs.iter().sum::<f64>() / tbs.len() as f64
            };
            mean_tbs.push(mean_tb);
            let cell = format!("beta={beta} n={n}");
            println!(
                "{cell}: conclusive {conclusive}/{SEEDS}, G median {median:?}, \
                 G>=1.5 {above}/{conclusive}, mean T_B {mean_tb:.2}, \
                 never delivered or never in all logs {incomplete}/{SEEDS}"
            );
            if tbs.is_empty() {
                failures.push(format!("{cell}: no seed delivered and reached every log"));
            }
            if conclusive < MIN_CONCLUSIVE {
                println!("  -> descriptive (allowance: < {MIN_CONCLUSIVE} conclusive)");
            } else if !(median.expect("conclusive nonempty") >= 1.5
                && (above as f64) >= 0.9 * conclusive as f64)
            {
                failures.push(cell);
            }
        }
        let xs: Vec<f64> = NS.iter().map(|&n| (n as f64).log2()).collect();
        let (slope, intercept, r2) = ols_fit(&xs, &mean_tbs).expect("fit");
        println!("arm beta={beta}: T_B ~ {slope:.3}*log2(n) + {intercept:.3}, R^2 = {r2:.4}");
        if !(slope > 0.0 && r2 >= 0.9) {
            failures.push(format!("arm beta={beta} T_B fit"));
        }
    }
    if failures.is_empty() {
        println!("E3 PASS — all cells within registered criteria");
    } else {
        println!("E3 FAIL — outside criteria: {failures:?}");
        std::process::exit(1);
    }
}
