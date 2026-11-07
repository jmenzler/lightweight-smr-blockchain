use sim::{BlockSchedule, Init, Scenario, run_logged, run_traced};

// Pre-registered gray-zone sweep (prereg/2026-07-21-gray-zone-sweep.md + Amendment 1).
// Usage: sweep <fresh|permanent|smoke> [out.csv]
// Holder-fraction series (P3) recorded for fresh arm, n=1000, beta in {0.135,0.140,0.145}.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arm = args.get(1).map(String::as_str).unwrap_or("fresh");
    let out = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| format!("sweep-{arm}.csv"));

    // betas in thousandths to keep grid points exact
    let (betas, ladder): (Vec<u32>, Vec<(usize, u64, u64)>) = match arm {
        "fresh" => (
            (0..=10).map(|i| 100 + i * 5).collect(),
            vec![(1000, 10000, 100), (4000, 20000, 50), (16000, 30000, 30)],
        ),
        "permanent" => (
            (0..=10).map(|i| 200 + i * 10).collect(),
            vec![(1000, 50000, 100), (4000, 60000, 50), (16000, 70000, 30)],
        ),
        "smoke" => (vec![100, 150], vec![(200, 10000, 3)]),
        other => panic!("arm must be fresh|permanent|smoke, got {other}"),
    };
    let schedule = |fraction: f64| match arm {
        "permanent" => BlockSchedule::Permanent { fraction },
        _ => BlockSchedule::FreshPerRound { fraction },
    };

    let mut csv = String::from("arm,n,beta,seed,outcome,rounds\n");
    let mut holders_csv = String::from("arm,n,beta,seed,round,holders\n");

    for &(n, seed_base, seed_count) in &ladder {
        for &beta_k in &betas {
            let beta = beta_k as f64 / 1000.0;
            let mut survived = 0u64;
            for i in 0..seed_count {
                let seed = seed_base + i;
                let scenario = Scenario {
                    cfg: sim::Config::default(),
                    n,
                    seed,
                    init: Init::Split { fraction: 0.5 },
                    max_rounds: 2000,
                    schedule: schedule(beta),
                    partition: None,
                };
                let record_holders =
                    arm == "fresh" && n == 1000 && matches!(beta_k, 135 | 140 | 145);
                let outcome = if record_holders {
                    let (outcome, trace) = run_traced(&scenario);
                    for (round, states) in trace.rounds.iter().enumerate() {
                        let holders = states.states.iter().flatten().count();
                        holders_csv.push_str(&format!(
                            "{arm},{n},{beta},{seed},{},{holders}\n",
                            round + 1
                        ));
                    }
                    outcome
                } else {
                    run_logged(&scenario, None).0
                };
                survived += u64::from(outcome.survived());
                let (label, rounds) = outcome.label();
                csv.push_str(&format!("{arm},{n},{beta},{seed},{label},{rounds}\n"));
            }
            eprintln!("{arm} n={n} beta={beta:.3}: {survived}/{seed_count} survived");
        }
    }

    std::fs::write(&out, csv).expect("write csv");
    if holders_csv.lines().count() > 1 {
        let holders_out = format!("{out}.holders.csv");
        std::fs::write(&holders_out, holders_csv).expect("write holders csv");
        eprintln!("wrote {holders_out}");
    }
    eprintln!("wrote {out}");
}
