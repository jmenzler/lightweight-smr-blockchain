//! Three-regime threshold bracket runner (Lemmas 2.1/2.2/2.3); run from the repo root with SIM_TRACE=off.

use sim::runlog::run_logged;
use sim::{Availability, BlockSchedule, Config, Init, Outcome, Scenario};

const SEEDS: u64 = 20;

#[derive(Clone, Copy, PartialEq)]
enum Regime {
    Collapse,
    ConvergeHold,
    DeathSpiral,
}

impl Regime {
    fn label(self) -> &'static str {
        match self {
            Regime::Collapse => "A-collapse",
            Regime::ConvergeHold => "B-converge-hold",
            Regime::DeathSpiral => "C-death-spiral",
        }
    }

    fn scenario(self, n: usize, seed: u64) -> Scenario {
        let split = Init::Split { fraction: 0.5 };
        let (init, schedule, max_rounds) = match self {
            Regime::Collapse => (
                Init::WithUndecided {
                    useful_fraction: 0.30,
                    inner: Box::new(split),
                },
                BlockSchedule::FreshPerRound { fraction: 0.0 },
                200,
            ),
            Regime::ConvergeHold => (
                Init::WithUndecided {
                    useful_fraction: 5.0 / 9.0,
                    inner: Box::new(split),
                },
                BlockSchedule::FreshPerRound { fraction: 0.10 },
                500,
            ),
            Regime::DeathSpiral => (split, BlockSchedule::Permanent { fraction: 0.30 }, 500),
        };
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
}

fn useful_per_round(metrics_csv: &str) -> Vec<u32> {
    std::fs::read_to_string(metrics_csv)
        .expect("metrics csv exists")
        .lines()
        .skip(1)
        .map(|l| l.split(',').nth(4).unwrap().parse().unwrap())
        .collect()
}

/// (round of the first crossing of 3n/4, whether useful stays above it, its minimum afterwards);
/// `None` when it never crosses.
fn hold_check(useful: &[u32], n: usize) -> Option<(usize, bool, u32)> {
    let threshold = (3 * n).div_ceil(4) as u32;
    let crossing = useful.iter().position(|&u| u >= threshold)?;
    let min = *useful[crossing..]
        .iter()
        .min()
        .expect("non-empty after crossing");
    Some((crossing + 1, min >= threshold, min))
}

fn main() {
    let mut failures: Vec<String> = Vec::new();
    for regime in [Regime::Collapse, Regime::ConvergeHold, Regime::DeathSpiral] {
        for (n_idx, n) in [1000usize, 4000].into_iter().enumerate() {
            let base = 80000
                + 1000
                    * (1 + match regime {
                        Regime::Collapse => 0,
                        Regime::ConvergeHold => 1,
                        Regime::DeathSpiral => 2,
                    })
                + 100 * n_idx as u64;
            let mut pass = 0;
            let mut death_rounds: Vec<usize> = Vec::new();
            let mut min_after_crossing: Vec<u32> = Vec::new();
            let mut in_precondition = 0usize;
            for i in 0..SEEDS {
                let scenario = regime.scenario(n, base + i);
                let extra = serde_json::json!({ "exp": "E2", "regime": regime.label() });
                let (outcome, metrics_path) = run_logged(&scenario, Some(extra));
                let ok = match (regime, &outcome) {
                    (Regime::Collapse | Regime::DeathSpiral, o) => {
                        let limit = if regime == Regime::Collapse { 30 } else { 300 };
                        o.extinction_round().is_some_and(|rounds| {
                            death_rounds.push(rounds);
                            rounds <= limit
                        })
                    }
                    (
                        Regime::ConvergeHold,
                        Outcome::Agreement {
                            value,
                            availability: Availability::Held,
                            ..
                        },
                    ) => {
                        assert!(*value < 2, "validity: value {value} not an input");
                        let metrics = metrics_path
                            .as_ref()
                            .expect("E2 reads the run ledger's metrics CSV; unset SIM_RUNLOG=off");
                        let useful = useful_per_round(metrics);
                        in_precondition += usize::from(2 * useful[0] as usize >= n);
                        match hold_check(&useful, n) {
                            Some((_, held, min)) => {
                                min_after_crossing.push(min);
                                held
                            }
                            None => false,
                        }
                    }
                    _ => false,
                };
                if ok {
                    pass += 1;
                }
            }
            let cell = format!("{} n={n}: {pass}/{SEEDS}", regime.label());
            let detail = if regime == Regime::ConvergeHold {
                format!(
                    "min useful after crossing: {:?} (3n/4 = {}); round-1 useful >= n/2 \
                     (Lemma 2.2's start) in {in_precondition} of the agreeing seeds",
                    min_after_crossing.iter().min(),
                    (3 * n).div_ceil(4)
                )
            } else {
                format!(
                    "death rounds min..max: {:?}..{:?}",
                    death_rounds.iter().min(),
                    death_rounds.iter().max()
                )
            };
            println!("{cell} — {detail}");

            let cell_ok = match (regime, n) {
                // Registered allowance: n=1000 hold is descriptive; ≥18/20 must agree and cross.
                (Regime::ConvergeHold, 1000) => min_after_crossing.len() >= 18,
                _ => pass == SEEDS as usize,
            };
            if !cell_ok {
                failures.push(cell);
            }
        }
    }
    if failures.is_empty() {
        println!("E2 PASS — all cells within registered criteria");
    } else {
        println!("E2 FAIL — cells outside criteria: {failures:?}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::hold_check;

    #[test]
    fn a_run_that_never_crosses_three_quarters_is_not_a_crash() {
        assert_eq!(hold_check(&[500, 600, 700, 740], 1000), None);
    }

    #[test]
    fn a_dip_after_the_crossing_fails_the_hold() {
        assert_eq!(
            hold_check(&[600, 760, 740, 800], 1000),
            Some((2, false, 740))
        );
    }
}
