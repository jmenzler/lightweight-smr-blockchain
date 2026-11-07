//! In-memory N-engine harness — the pre-Shadow rehearsal. Perfect network:
//! every emitted message is delivered instantly within the round. Property
//! tests pin protocol invariants; the `#[ignore]` distribution test compares
//! convergence-round histograms against `sim::run` (E7's bar, seed-swept).

use network_node::harness::run;
use network_node::record::{PostState, RoundRecord};
use network_node::spec::NodeRunSpec;

fn median_json(n: usize, seed: u64, rounds: usize) -> String {
    format!(
        r#"{{"scenario":{{"median":{{"n":{n},"seed":{seed},"k":6,"ell":3,
             "init":{{"kind":"distinct"}},"max_rounds":{rounds}}}}},"sync":"timer"}}"#
    )
}

fn values_at(records: &[Vec<RoundRecord>], round_idx: usize) -> Vec<Option<u64>> {
    records
        .iter()
        .map(|r| match r[round_idx].post {
            PostState::Value(v) => v,
            _ => panic!("value mode"),
        })
        .collect()
}

#[test]
fn unanimity_is_absorbing() {
    let spec = NodeRunSpec::from_json(&median_json(16, 3, 60)).expect("spec");
    let records = run(&spec);
    let rounds = records[0].len();
    let mut agreed_at = None;
    for r in 0..rounds {
        let vals = values_at(&records, r);
        let held: Vec<u64> = vals.iter().filter_map(|v| *v).collect();
        if !held.is_empty() && held.iter().all(|v| *v == held[0]) {
            agreed_at = Some((r, held[0]));
            break;
        }
    }
    let (first, value) = agreed_at.expect("n=16 distinct converges within 60 rounds");
    for r in first..rounds {
        let vals = values_at(&records, r);
        for v in vals.iter().filter_map(|v| *v) {
            assert_eq!(v, value, "round {r}: unanimity must persist (Lemma-level)");
        }
    }
}

#[test]
fn no_value_is_ever_invented() {
    let spec = NodeRunSpec::from_json(&median_json(16, 8, 40)).expect("spec");
    let records = run(&spec);
    let mut alive: Vec<u64> = (0..16).collect(); // distinct init: node i holds i
    for r in 0..records[0].len() {
        let now: Vec<u64> = values_at(&records, r).iter().filter_map(|v| *v).collect();
        for v in &now {
            assert!(
                alive.contains(v),
                "round {r}: value {v} appeared from nowhere (validity)"
            );
        }
        alive = now;
        if alive.is_empty() {
            break;
        }
    }
}

#[test]
fn rerun_is_bit_identical() {
    let spec = NodeRunSpec::from_json(&median_json(12, 21, 30)).expect("spec");
    assert_eq!(run(&spec), run(&spec), "DP-8: same spec, same everything");
}

#[test]
fn distinct_seeds_diverge() {
    let a = NodeRunSpec::from_json(&median_json(12, 1, 30)).expect("spec");
    let b = NodeRunSpec::from_json(&median_json(12, 2, 30)).expect("spec");
    assert_ne!(run(&a), run(&b), "seed must matter");
}

/// Holders (non-⊥ servers) after rounds 1, 2 and 3, engine harness vs sim, over a seed sweep
/// with blocking. The tiers draw from different streams, so trajectories differ per seed; the
/// per-round holder-count distributions must agree.
#[test]
fn blocked_run_holder_counts_match_sim() {
    let (n, seeds, rounds) = (32usize, 150u64, [0usize, 1, 2]);
    let cases = [
        (
            r#"{"kind":"fresh_per_round","fraction":0.1}"#,
            sim::BlockSchedule::FreshPerRound { fraction: 0.1 },
        ),
        (
            r#"{"kind":"fresh_per_round","fraction":0.25}"#,
            sim::BlockSchedule::FreshPerRound { fraction: 0.25 },
        ),
        (
            r#"{"kind":"permanent","fraction":0.2}"#,
            sim::BlockSchedule::Permanent { fraction: 0.2 },
        ),
    ];
    for (schedule_json, schedule) in cases {
        let mut engine: Vec<Vec<f64>> = vec![Vec::new(); rounds.len()];
        let mut simulated: Vec<Vec<f64>> = vec![Vec::new(); rounds.len()];
        for seed in 0..seeds {
            let spec = NodeRunSpec::from_json(&format!(
                r#"{{"scenario":{{"median":{{"n":{n},"seed":{seed},"k":6,"ell":3,
                    "init":{{"kind":"distinct"}},"max_rounds":6,
                    "schedule":{schedule_json}}}}},"sync":"timer"}}"#
            ))
            .expect("spec");
            let records = run(&spec);
            let (_, trace) = sim::run_traced(&sim::Scenario {
                n,
                seed,
                cfg: protocol::Config::default(),
                init: sim::Init::Distinct,
                max_rounds: 6,
                schedule: schedule.clone(),
                partition: None,
            });
            for (slot, &r) in rounds.iter().enumerate() {
                engine[slot].push(values_at(&records, r).iter().flatten().count() as f64);
                simulated[slot].push(trace.rounds[r].states.iter().flatten().count() as f64);
            }
        }
        let mean_var = |v: &[f64]| {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            let var = v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64;
            (m, var)
        };
        for (slot, &r) in rounds.iter().enumerate() {
            let (me, ve) = mean_var(&engine[slot]);
            let (ms, vs) = mean_var(&simulated[slot]);
            let se = (ve / seeds as f64 + vs / seeds as f64).sqrt().max(1e-9);
            assert!(
                (me - ms).abs() <= 5.0 * se,
                "{schedule_json}, after round {}: engine mean {me:.2} vs sim mean {ms:.2} holders (SE {se:.2})",
                r + 1
            );
        }
    }
}

/// Convergence round under the sim's rule: first round of holder-unanimity
/// that persists to the horizon; None if never (or extinct).
fn convergence_round(records: &[Vec<RoundRecord>]) -> Option<usize> {
    let rounds = records[0].len();
    let mut candidate = None;
    for r in 0..rounds {
        let vals = values_at(records, r);
        let held: Vec<u64> = vals.iter().filter_map(|v| *v).collect();
        let unanimous = !held.is_empty() && held.iter().all(|v| *v == held[0]);
        match (unanimous, candidate) {
            (true, None) => candidate = Some((r + 1, held[0])),
            (true, Some((_, v))) if held[0] != v => candidate = Some((r + 1, held[0])),
            (false, Some(_)) => candidate = None,
            _ => {}
        }
    }
    candidate.map(|(r, _)| r)
}

/// E7 rehearsal: engine-harness vs sim convergence-round distributions at
/// n=64 over a seed sweep. Slow — run explicitly:
/// `cargo test -p network-node --test in_process harness:: -- --ignored`
#[test]
#[ignore]
fn convergence_distribution_matches_sim_at_n64() {
    let n = 64;
    let horizon = 120;
    let seeds = 200u64;

    let mut engine_rounds = Vec::new();
    let mut sim_rounds = Vec::new();
    for seed in 0..seeds {
        let spec = NodeRunSpec::from_json(&median_json(n, seed, horizon)).expect("spec");
        if let Some(r) = convergence_round(&run(&spec)) {
            engine_rounds.push(r as f64);
        }

        let scenario = sim::Scenario {
            n,
            seed,
            cfg: protocol::Config::default(),
            init: sim::Init::Distinct,
            max_rounds: horizon,
            schedule: sim::BlockSchedule::FreshPerRound { fraction: 0.0 },
            partition: None,
        };
        if let sim::Outcome::Agreement { rounds, .. } = sim::run(&scenario) {
            sim_rounds.push(rounds as f64);
        }
    }
    assert!(
        engine_rounds.len() as u64 > seeds * 9 / 10,
        "engine side converges"
    );
    assert!(
        sim_rounds.len() as u64 > seeds * 9 / 10,
        "sim side converges"
    );

    // Two-sample KS statistic, coarse tolerance (200 vs 200 → crit ≈ 0.163
    // at α=0.01; we allow 0.2 — this is a rehearsal gate, the real E7 test
    // is preregistered with its own statistics).
    let ks = {
        let mut a = engine_rounds.clone();
        let mut b = sim_rounds.clone();
        a.sort_by(f64::total_cmp);
        b.sort_by(f64::total_cmp);
        let mut d: f64 = 0.0;
        let all: Vec<f64> = a.iter().chain(b.iter()).copied().collect();
        for x in all {
            let fa = a.iter().filter(|v| **v <= x).count() as f64 / a.len() as f64;
            let fb = b.iter().filter(|v| **v <= x).count() as f64 / b.len() as f64;
            d = d.max((fa - fb).abs());
        }
        d
    };
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    assert!(
        ks < 0.2,
        "KS {ks:.3} too large — engine mean {:.1} vs sim mean {:.1}",
        mean(&engine_rounds),
        mean(&sim_rounds)
    );
}
