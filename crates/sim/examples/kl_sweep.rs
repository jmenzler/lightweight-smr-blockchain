// (k,ℓ)-generalization of the gray-zone liveness sweep — runner-only, protocol
// crates untouched (mirrors the horizon-arm discipline). For each (k,ℓ) pair it
// locates the fresh-per-round survival boundary by bisection-free grid scan and
// reports, per pair: the last-100%-survive and first-0%-survive β (grid step
// 0.005), plus median agreement/death rounds at the edge. A run whose fate the horizon
// cut off (still mixed, or agreed with fewer than n/3 holders) is neither: it is counted in
// `censored` (`collapsing` names the latter) and kept out of death_median. Init is the registered
// Split{0.5}, all-holding, max_rounds=2000 — identical to Arm A of the C2 sweep,
// only (k,ℓ) varies. Deterministic: ChaCha12, disjoint seed base per (k,ℓ).
//
// Usage: kl_sweep <fresh|permanent> [out.csv] [n] [seeds]
use sim::analysis::SurvivalTally;
use sim::{BlockSchedule, Config, Init, Scenario, run_logged};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let arm = a.get(1).map(String::as_str).unwrap_or("fresh");
    let out = a
        .get(2)
        .cloned()
        .unwrap_or_else(|| format!("kl-sweep-{arm}.csv"));
    let n: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let seeds: u64 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(40);

    // (k, ℓ, seed slot); Env KL_PAIRS="8:3,10:3" overrides the default list for targeted runs.
    // Default slots keep the original 14-pair order (even-ℓ pairs never ran), so reruns
    // reproduce the ledger's seed bases.
    let pairs: Vec<(usize, usize, u64)> = match std::env::var("KL_PAIRS") {
        Ok(s) => s
            .split(',')
            .zip(0..)
            .map(|(p, slot)| {
                let (k, ell) = p
                    .split_once(':')
                    .and_then(|(k, ell)| Some((k.parse().ok()?, ell.parse().ok()?)))
                    .unwrap_or_else(|| panic!("KL_PAIRS entry {p:?}; expected k:ell"));
                (k, ell, slot)
            })
            .collect(),
        Err(_) => vec![
            (3, 3, 1),
            (4, 3, 2),
            (5, 3, 3),
            (6, 3, 4),
            (6, 5, 6),
            (8, 3, 7),
            (8, 5, 8),
            (10, 3, 9),
            (10, 5, 10),
            (12, 3, 11),
            (12, 5, 12),
            (12, 7, 13),
        ],
    };
    let cfgs: Vec<Config> = pairs
        .iter()
        .map(|&(k, ell, _)| {
            Config::new(k, ell).unwrap_or_else(|e| panic!("invalid (k,ell)=({k},{ell}): {e}"))
        })
        .collect();

    // β grid in thousandths. Env KL_BETA_LO/HI/STEP narrow the scan window so a
    // targeted confirmation run doesn't scan the whole [0.01,0.60] range.
    let lo: u32 = std::env::var("KL_BETA_LO")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let hi: u32 = std::env::var("KL_BETA_HI")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let step: usize = std::env::var("KL_BETA_STEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let betas: Vec<u32> = (lo..=hi).step_by(step).collect();

    let mut csv = String::from(
        "arm,k,ell,n,beta,seeds,survived,agree_median,death_median,censored,collapsing\n",
    );
    for (&(k, ell, slot), &cfg) in pairs.iter().zip(&cfgs) {
        let seed_base = 900_000 + slot * 10_000;
        let mut last_100: Option<f64> = None;
        let mut first_0: Option<f64> = None;
        for &bk in &betas {
            let beta = bk as f64 / 1000.0;
            let mut tally = SurvivalTally::default();
            for i in 0..seeds {
                let scenario = Scenario {
                    cfg,
                    n,
                    seed: seed_base + i,
                    init: Init::Split { fraction: 0.5 },
                    max_rounds: 2000,
                    schedule: match arm {
                        "permanent" => BlockSchedule::Permanent { fraction: beta },
                        _ => BlockSchedule::FreshPerRound { fraction: beta },
                    },
                    partition: None,
                };
                tally.add(&run_logged(&scenario, None).0);
            }
            let med = |v: &mut Vec<usize>| {
                if v.is_empty() {
                    -1i64
                } else {
                    v.sort_unstable();
                    v[v.len() / 2] as i64
                }
            };
            let am = med(&mut tally.agree_rounds);
            let dm = med(&mut tally.death_rounds);
            let SurvivalTally {
                survived,
                censored,
                collapsing,
                ..
            } = tally;
            csv.push_str(&format!(
                "{arm},{k},{ell},{n},{beta:.3},{seeds},{survived},{am},{dm},{censored},{collapsing}\n"
            ));
            if survived == seeds {
                last_100 = Some(beta);
            }
            if tally.none_survive() && first_0.is_none() && last_100.is_some() {
                first_0 = Some(beta);
            }
            // Stops the scan at the first β where no seed survives and one died. Survival falls with
            // β only in expectation, so a seed batch can hit zero survivors before a higher β shows some.
            if tally.none_survive() {
                break;
            }
        }
        eprintln!(
            "({k},{ell}) {arm}: last100%={:?} first0%={:?}",
            last_100, first_0
        );
    }
    std::fs::write(&out, csv).expect("write csv");
    eprintln!("wrote {out}");
}
