//! Gossip and priority modes over whole runs: initial placement, one-round spreading laws,
//! and the blocked / below-ℓ rules over every round of a blocked run. Perfect network, so the
//! only randomness is the targets and the subset draws.

use network_node::engine::RoundEngine;
use network_node::harness::run;
use network_node::record::{PostState, RoundRecord};
use network_node::spec::NodeRunSpec;
use network_node::wire::{Msg, ReplyPayload};

const X: u64 = 7;
const X0: u64 = 0;

fn gossip_spec(n: usize, seed: u64, holders: usize, rounds: usize, beta: f64) -> NodeRunSpec {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"gossip":{{"n":{n},"seed":{seed},"k":6,"ell":3,"holders":{holders},
            "x":{X},"x0":{X0},"max_rounds":{rounds},
            "schedule":{{"kind":"fresh_per_round","fraction":{beta}}}}}}},"sync":"timer"}}"#
    ))
    .expect("spec")
}

fn priority_spec(n: usize, seed: u64, rounds: usize, beta: f64) -> NodeRunSpec {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"priority":{{"n":{n},"seed":{seed},"k":6,"ell":3,
            "init":{{"kind":"split","fraction":0.5}},"max_rounds":{rounds},
            "schedule":{{"kind":"fresh_per_round","fraction":{beta}}}}}}},"sync":"timer"}}"#
    ))
    .expect("spec")
}

fn value_of(record: &RoundRecord) -> Option<u64> {
    match record.post {
        PostState::Value(v) => v,
        _ => panic!("value mode"),
    }
}

/// Number of servers holding `v` after round `round_idx` (0-based).
fn holding(records: &[Vec<RoundRecord>], round_idx: usize, v: u64) -> usize {
    records
        .iter()
        .filter(|r| value_of(&r[round_idx]) == Some(v))
        .count()
}

/// The value a fresh engine serves when pulled in round 1.
fn served_value(spec: &NodeRunSpec, id: u32) -> Option<u64> {
    let mut e = RoundEngine::new(spec, id).expect("engine");
    e.on_round_start();
    let out = e.on_message(Msg::PullRequest {
        round: 1,
        from: 1,
        slot: 0,
        needs_state: false,
    });
    match out.first() {
        Some((
            _,
            Msg::PullReply {
                payload: ReplyPayload::Value(v),
                ..
            },
        )) => Some(*v),
        _ => None,
    }
}

#[test]
fn gossip_starts_exactly_the_first_holders_with_x() {
    // Alg 4: "a subset of servers start with x_i = x, all others with x_i = x0"
    let spec = gossip_spec(16, 1, 5, 3, 0.0);
    for id in 0..16u32 {
        let expected = if id < 5 { X } else { X0 };
        assert_eq!(served_value(&spec, id), Some(expected), "server {id}");
    }
}

/// Mean over seeds of the number of servers holding `v` after round 1.
fn mean_after_round_one(spec_of: impl Fn(u64) -> NodeRunSpec, v: u64, seeds: u64) -> f64 {
    let total: usize = (0..seeds)
        .map(|seed| holding(&run(&spec_of(seed)), 0, v))
        .sum();
    total as f64 / seeds as f64
}

/// `|mean - n·q|` within 5 standard errors of a Binomial(n, q) mean over `seeds` runs.
fn assert_binomial_mean(mean: f64, n: usize, q: f64, seeds: u64) {
    let expected = n as f64 * q;
    let tolerance = 5.0 * (n as f64 * q * (1.0 - q) / seeds as f64).sqrt();
    assert!(
        (mean - expected).abs() <= tolerance,
        "mean {mean:.2}, expected {expected:.2} (tolerance {tolerance:.2})"
    );
}

#[test]
fn gossip_holders_after_one_round_follow_one_minus_miss_to_the_k() {
    // Alg 4 at β = 0: a server holds x after the round iff one of its k = 6 uniform requests hit
    // a holder, so each of the n servers is a holder with probability 1 − (1 − γ)^6, γ the
    // holder fraction. Lemma 3.1's growth factor 3/2 for γ ≤ 1/3 follows.
    let n = 64;
    for holders in [4usize, 8, 16] {
        let gamma = holders as f64 / n as f64;
        let q = 1.0 - (1.0 - gamma).powi(6);
        assert!(q >= 1.5 * gamma, "paper's growth bound at γ = {gamma}");
        let seeds = 200;
        let mean = mean_after_round_one(|s| gossip_spec(n, s, holders, 1, 0.0), X, seeds);
        assert_binomial_mean(mean, n, q, seeds);
    }
}

#[test]
fn priority_holders_of_the_maximum_after_one_round_follow_one_minus_miss_to_the_ell() {
    // Def 3.5 at β = 0: of the ℓ = 3 replies in the uniform subset, each carries the maximum
    // with probability γ, so a server adopts it with probability 1 − (1 − γ)^3. Split 0.5 gives
    // the maximum 1 to the 32 of 64 servers with ids 32..64: q = 1 − (1/2)^3 = 7/8.
    let seeds = 200;
    let mean = mean_after_round_one(|s| priority_spec(64, s, 1, 0.0), 1, seeds);
    assert_binomial_mean(mean, 64, 0.875, seeds);
}

#[test]
fn gossip_with_many_holders_reaches_every_server() {
    // Thm 4: from c·log n useful holders the value spreads to all within O(log n) rounds.
    // n = 64, 30 holders, β = 0: all 64 servers hold x within 4·log₂ n = 24 rounds, every seed.
    for seed in 0..40 {
        let records = run(&gossip_spec(64, seed, 30, 24, 0.0));
        assert_eq!(holding(&records, 23, X), 64, "seed {seed}");
    }
}

/// Every round of every node obeys the blocked / below-ℓ / request rules of Alg 1, 2 and 4.
fn assert_round_rules(records: &[Vec<RoundRecord>], k: u32, ell: u8) {
    let mut checked_below_ell = 0;
    let mut checked_bottom_requests = 0;
    for (id, node) in records.iter().enumerate() {
        for (idx, rec) in node.iter().enumerate() {
            let post = value_of(rec);
            if rec.blocked {
                assert_eq!(post, None, "server {id} round {}: blocked → ⊥", rec.r);
            } else if rec.replies < u16::from(ell) {
                assert_eq!(post, None, "server {id} round {}: < ℓ replies → ⊥", rec.r);
                checked_below_ell += 1;
            } else {
                assert!(post.is_some(), "server {id} round {}: ≥ ℓ replies", rec.r);
            }
            let was_bottom = idx > 0 && value_of(&node[idx - 1]).is_none();
            if was_bottom && !rec.blocked {
                assert_eq!(
                    rec.req_sent, k,
                    "server {id} round {}: a ⊥ server still sends k requests",
                    rec.r
                );
                checked_bottom_requests += 1;
            }
        }
    }
    assert!(checked_below_ell > 0, "the run must exercise the < ℓ rule");
    assert!(
        checked_bottom_requests > 0,
        "the run must exercise ⊥ servers sending requests"
    );
}

#[test]
fn gossip_blocked_and_below_ell_servers_are_bottom_and_bottom_servers_still_ask() {
    assert_round_rules(&run(&gossip_spec(64, 5, 30, 12, 0.2)), 6, 3);
}

#[test]
fn priority_blocked_and_below_ell_servers_are_bottom_and_bottom_servers_still_ask() {
    assert_round_rules(&run(&priority_spec(64, 5, 12, 0.2)), 6, 3);
}
