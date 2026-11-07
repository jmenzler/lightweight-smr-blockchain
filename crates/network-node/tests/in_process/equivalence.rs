//! Node-vs-sim equivalence for the SMR arms: the in-process engines (perfect delivery, one client
//! command resent to a uniform server each round) against the sim, compared as distributions.
//! The two tiers draw from different RNG streams, so only the laws can agree.

use network_node::collect::aggregate_smr;
use network_node::engine::RoundEngine;
use network_node::record::RoundRecord;
use network_node::spec::NodeRunSpec;
use network_node::wire::{AckKind, Msg};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;
use sim::smr::{SmrScenario, run_smr};
use sim::spec::SmrScenarioSpec;

const OP: u64 = 5;
const EXTENDED: &str = r#"{"kind":"extended"}"#;

struct Setup {
    n: usize,
    seeds: u64,
    horizon: usize,
    schedule: &'static str,
    /// Whether the schedule blocks servers: only then does the client's resend make delivery a law.
    blocks: bool,
}

const SMALL: Setup = Setup {
    n: 32,
    seeds: 60,
    horizon: 40,
    schedule: r#"{"kind":"fresh_per_round","fraction":0.0}"#,
    blocks: false,
};

fn compact(t: u64) -> String {
    format!(r#"{{"kind":"compact","t_commit_rounds":{t}}}"#)
}

fn smr_json(setup: &Setup, seed: u64, proto: &str) -> String {
    format!(
        r#"{{"n":{},"seed":{seed},"k":6,"ell":3,"sigma":1.0,"proto":{proto},
            "injections":[{{"round":1,"client":1,"op":{OP}}}],"max_rounds":{},
            "schedule":{}}}"#,
        setup.n, setup.horizon, setup.schedule
    )
}

#[derive(Default)]
struct Outcome {
    t_b: Option<u64>,
    t_e: Option<u64>,
    ack: Option<u64>,
    /// Appends the nodes sent over the run; the sim reports none, so only the node side fills it.
    appends: u64,
}

fn node_run(setup: &Setup, seed: u64, proto: &str, terminal: AckKind) -> Outcome {
    let spec = NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"smr":{}}},"sync":"timer","tracked_ops":[{OP}]}}"#,
        smr_json(setup, seed, proto)
    ))
    .expect("node spec");
    let n = setup.n;
    let mut engines: Vec<RoundEngine> = (0..n as u32)
        .map(|id| RoundEngine::new(&spec, id).expect("engine"))
        .collect();
    let mut records: Vec<Vec<RoundRecord>> = vec![Vec::new(); n];
    let mut target_rng = ChaCha12Rng::seed_from_u64(seed ^ 0x5eed);
    let mut ack = None;
    for round in 1..=setup.horizon as u64 {
        let mut in_flight: Vec<(u32, Msg)> = Vec::new();
        for e in engines.iter_mut() {
            in_flight.extend(e.on_round_start());
        }
        if ack.is_none() {
            let target = target_rng.random_range(0..n as u32);
            in_flight.push((
                target,
                Msg::ClientCmd {
                    round,
                    client: 1,
                    sn: 1,
                    op: OP,
                },
            ));
        }
        while let Some((to, m)) = in_flight.pop() {
            if let Msg::ClientAck { kind, .. } = m {
                if kind == terminal && ack.is_none() {
                    ack = Some(round);
                }
                continue;
            }
            in_flight.extend(engines[to as usize].on_message(m));
        }
        for (id, e) in engines.iter_mut().enumerate() {
            records[id].push(e.on_round_end());
        }
    }
    let out = aggregate_smr(&records, &spec, None).expect("aggregate");
    let l = out
        .landmarks
        .iter()
        .find(|l| l.op == OP)
        .expect("tracked landmark");
    Outcome {
        t_b: l.all_logs_round,
        t_e: l.prefix_fixed_round,
        ack,
        appends: records
            .iter()
            .flatten()
            .map(|r| u64::from(r.app_sent))
            .sum(),
    }
}

fn sim_run(setup: &Setup, seed: u64, proto: &str, extended: bool) -> Outcome {
    let spec: SmrScenarioSpec =
        serde_json::from_str(&smr_json(setup, seed, proto)).expect("sim spec");
    let report = run_smr(&SmrScenario::try_from(spec).expect("scenario"));
    let c = report
        .commands
        .iter()
        .find(|c| c.op == OP)
        .expect("tracked command");
    Outcome {
        t_b: c.all_logs_round.map(|r| r as u64),
        t_e: c.prefix_fixed_round.map(|r| r as u64),
        ack: if extended {
            c.delivered_round
        } else {
            c.committed_ack_round
        }
        .map(|r| r as u64),
        appends: 0,
    }
}

fn ks(a: &[f64], b: &[f64]) -> f64 {
    let (mut a, mut b) = (a.to_vec(), b.to_vec());
    a.sort_by(f64::total_cmp);
    b.sort_by(f64::total_cmp);
    let cdf = |v: &[f64], x: f64| v.iter().filter(|y| **y <= x).count() as f64 / v.len() as f64;
    a.iter()
        .chain(b.iter())
        .map(|&x| (cdf(&a, x) - cdf(&b, x)).abs())
        .fold(0.0, f64::max)
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

struct Samples {
    node: Vec<Outcome>,
    sim: Vec<Outcome>,
}

fn sample(setup: &Setup, proto: &str, terminal: AckKind) -> Samples {
    let extended = terminal == AckKind::Delivered;
    Samples {
        node: (0..setup.seeds)
            .map(|s| node_run(setup, s, proto, terminal))
            .collect(),
        sim: (0..setup.seeds)
            .map(|s| sim_run(setup, s, proto, extended))
            .collect(),
    }
}

fn column(runs: &[Outcome], f: fn(&Outcome) -> Option<u64>) -> Vec<f64> {
    runs.iter().filter_map(f).map(|r| r as f64).collect()
}

fn assert_same_distribution(label: &str, seeds: u64, node: &[f64], sim: &[f64], ks_max: f64) {
    assert!(
        node.len().abs_diff(sim.len()) as u64 <= seeds / 10,
        "{label}: defined in {} of {seeds} seeds on the node but {} of {seeds} in the sim",
        node.len(),
        sim.len()
    );
    let d = ks(node, sim);
    assert!(
        d < ks_max && (mean(node) - mean(sim)).abs() < 0.5,
        "{label}: node vs sim differ: KS {d:.3}, node mean {:.2}, sim mean {:.2}",
        mean(node),
        mean(sim)
    );
}

fn check_extended(setup: &Setup, ks_max: f64) {
    let s = sample(setup, EXTENDED, AckKind::Delivered);
    let mut columns = vec![
        ("extended T_B", (|o| o.t_b) as fn(&Outcome) -> Option<u64>),
        ("extended T_E", |o| o.t_e),
    ];
    if setup.blocks {
        // without blocking the first server reached always delivers in round 1, on both sides
        columns.push(("extended delivery", |o| o.ack));
    }
    for (label, f) in columns {
        assert_same_distribution(
            label,
            setup.seeds,
            &column(&s.node, f),
            &column(&s.sim, f),
            ks_max,
        );
    }
    if !setup.blocks {
        // one command, delivered once, is amplified to exactly ceil(σ·log₂ n) servers and never again
        let fan_out = sim::smr::amp_count(1.0, setup.n) as u64;
        for (seed, run) in s.node.iter().enumerate() {
            assert_eq!(
                run.appends, fan_out,
                "seed {seed}: appends sent for one command"
            );
        }
    }
}

fn check_compact(setup: &Setup, ks_max: f64) {
    // T far above the horizon: nothing is committed, so T_B is plain log coverage on both sides
    let s = sample(
        setup,
        &compact(setup.horizon as u64 + 10),
        AckKind::Delivered,
    );
    assert_same_distribution(
        "compact T_B (no commit in horizon)",
        setup.seeds,
        &column(&s.node, |o| o.t_b),
        &column(&s.sim, |o| o.t_b),
        ks_max,
    );
    // T = 4: the commit round of the first server to execute it, seen through the client's ack
    let s = sample(setup, &compact(4), AckKind::AckCommitted);
    assert_same_distribution(
        "compact commit ack (T = 4)",
        setup.seeds,
        &column(&s.node, |o| o.ack),
        &column(&s.sim, |o| o.ack),
        ks_max,
    );
    // a command committed before it covered every log never latches on either side
    assert_same_distribution(
        "compact T_B (T = 4)",
        setup.seeds,
        &column(&s.node, |o| o.t_b),
        &column(&s.sim, |o| o.t_b),
        ks_max,
    );
}

// p. 20: the extended arm on node engines matches the sim's T_B, T_E and delivery distributions.
#[test]
fn extended_node_and_sim_landmark_distributions_agree() {
    check_extended(&SMALL, 0.25);
}

// p. 26: the compact arm on node engines matches the sim's coverage and commit-ack distributions.
#[test]
fn compact_node_and_sim_distributions_agree() {
    check_compact(&SMALL, 0.25);
}

#[test]
#[ignore = "slow: 300 seeds at n = 64"]
fn extended_distributions_agree_at_larger_n_and_sample() {
    let setup = Setup {
        n: 64,
        seeds: 300,
        horizon: 60,
        ..SMALL
    };
    check_extended(&setup, 0.15);
}

#[test]
#[ignore = "slow: 300 seeds at n = 64"]
fn compact_distributions_agree_at_larger_n_and_sample() {
    let setup = Setup {
        n: 64,
        seeds: 300,
        horizon: 60,
        ..SMALL
    };
    check_compact(&setup, 0.15);
}

#[test]
#[ignore = "slow: blocked servers, 300 seeds"]
fn extended_distributions_agree_under_fresh_per_round_blocking() {
    let setup = Setup {
        n: 32,
        seeds: 300,
        horizon: 60,
        schedule: r#"{"kind":"fresh_per_round","fraction":0.2}"#,
        blocks: true,
    };
    check_extended(&setup, 0.15);
}
