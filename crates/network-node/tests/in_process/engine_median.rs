//! RoundEngine semantics table for the three Alg-1-shaped modes
//! (median / gossip / priority) — checklist §4 shared rows. The SMR arms
//! (extended/compact) land in a later commit with their own suite.

use network_node::engine::RoundEngine;
use network_node::record::PostState;
use network_node::spec::NodeRunSpec;
use network_node::wire::{Msg, ReplyPayload};

fn median_json(n: usize, seed: u64, rounds: usize, schedule: &str) -> String {
    let sched = if schedule.is_empty() {
        String::new()
    } else {
        format!(r#","schedule":{schedule}"#)
    };
    format!(
        r#"{{"scenario":{{"median":{{"n":{n},"seed":{seed},"k":6,"ell":3,
             "init":{{"kind":"distinct"}},"max_rounds":{rounds}{sched}}}}},"sync":"timer"}}"#
    )
}

fn engines(spec: &NodeRunSpec) -> Vec<RoundEngine> {
    (0..spec.n() as u32)
        .map(|id| RoundEngine::new(spec, id).expect("engine"))
        .collect()
}

/// Route every emitted message to its destination until quiescent.
fn run_round(engines: &mut [RoundEngine]) {
    let mut in_flight: Vec<(u32, Msg)> = Vec::new();
    for e in engines.iter_mut() {
        in_flight.extend(e.on_round_start());
    }
    while let Some((to, m)) = in_flight.pop() {
        in_flight.extend(engines[to as usize].on_message(m));
    }
}

fn post_value(rec: &network_node::record::RoundRecord) -> Option<u64> {
    match rec.post {
        PostState::Value(v) => v,
        _ => panic!("value-mode post"),
    }
}

// --- self-sampling (F12) ---------------------------------------------------

#[test]
fn n1_self_pulls_keep_the_holder_forever() {
    // n=1: all k pulls hit self via loopback; 6 replies of the own value keep
    // the holder decided round after round — pins self-serve + snapshot path.
    let spec = NodeRunSpec::from_json(&median_json(1, 5, 5, "")).expect("spec");
    let mut e = engines(&spec);
    for _ in 0..5 {
        run_round(&mut e);
        let rec = e[0].on_round_end();
        assert_eq!(post_value(&rec), Some(0), "distinct init: node 0 holds 0");
        assert_eq!(rec.replies, 6, "all six self-replies collected");
    }
}

// --- blocking --------------------------------------------------------------

fn window_block_node0() -> &'static str {
    r#"{"kind":"windows","windows":[{"start_round":1,"rounds":1,
        "target":{"kind":"nodes","ids":[0]}}]}"#
}

#[test]
fn blocked_node_sends_nothing_serves_nothing_ends_undecided() {
    let spec = NodeRunSpec::from_json(&median_json(4, 9, 3, window_block_node0())).expect("spec");
    let mut e = engines(&spec);
    run_round(&mut e);
    let rec0 = e[0].on_round_end();
    assert!(rec0.blocked);
    assert_eq!(rec0.req_sent, 0, "blocked: no pulls out");
    assert_eq!(rec0.rep_sent, 0, "blocked: serves nobody");
    assert_eq!(post_value(&rec0), None, "blocked round ends undecided");
}

#[test]
fn blocked_round_still_consumes_the_target_draws() {
    // The k target draws happen even when blocked. With no replies in either arm the step takes no
    // ℓ-subset draw, so round 2's targets match; an open node that receives >= ℓ replies draws
    // more, and its stream then differs from a blocked node's.
    let blocked = NodeRunSpec::from_json(&median_json(4, 9, 3, window_block_node0())).expect("s");
    let free = NodeRunSpec::from_json(&median_json(4, 9, 3, "")).expect("s");
    let mut eb = RoundEngine::new(&blocked, 0).expect("engine");
    let mut ef = RoundEngine::new(&free, 0).expect("engine");

    assert!(eb.on_round_start().is_empty(), "blocked round 1: silent");
    let round1_free = ef.on_round_start();
    assert!(!round1_free.is_empty());
    eb.on_round_end();
    ef.on_round_end();

    let t2b: Vec<u32> = eb.on_round_start().iter().map(|(to, _)| *to).collect();
    let t2f: Vec<u32> = ef.on_round_start().iter().map(|(to, _)| *to).collect();
    assert_eq!(t2b, t2f, "round-2 draws unaffected by round-1 blocking");
}

// --- reply-count thresholds ------------------------------------------------

/// Feed crafted replies straight into slots (bypassing real pulls).
fn craft_replies(e: &mut RoundEngine, round: u64, values: &[u64]) {
    for (slot, v) in values.iter().enumerate() {
        e.on_message(Msg::PullReply {
            round,
            from: 1,
            slot: slot as u16,
            payload: ReplyPayload::Value(*v),
        });
    }
}

fn one_engine(json: &str) -> RoundEngine {
    let spec = NodeRunSpec::from_json(json).expect("spec");
    let mut e = RoundEngine::new(&spec, 0).expect("engine");
    e.on_round_start();
    e
}

#[test]
fn fewer_than_ell_replies_forces_bot_more_than_ell_adopts() {
    let mut e = one_engine(&median_json(8, 2, 9, ""));
    craft_replies(&mut e, 1, &[5, 9]);
    assert_eq!(post_value(&e.on_round_end()), None, "2 < ℓ=3 → ⊥");

    e.on_round_start();
    craft_replies(&mut e, 2, &[5, 9, 9, 7, 5, 1]);
    let got = e.on_round_end();
    let v = post_value(&got).expect("≥ℓ replies re-decide the node");
    assert!(
        [5, 9, 7].contains(&v),
        "adopted ∈ replies, and 1 cannot be the median of any 3 of them, got {v}"
    );
}

const TRIALS: u64 = 4000;

/// The value `mode_json(seed)`'s node 0 adopts from crafted round-1 `replies`, for each seed.
fn adopted_over_seeds(mode_json: impl Fn(u64) -> String, replies: &[u64]) -> Vec<u64> {
    (0..TRIALS)
        .map(|seed| {
            let mut e = one_engine(&mode_json(seed));
            craft_replies(&mut e, 1, replies);
            post_value(&e.on_round_end()).expect("at least ℓ replies")
        })
        .collect()
}

/// `law = [(value, probability)]`: a probability-0 value must never occur; every other
/// frequency must lie within 5 binomial standard deviations.
fn assert_follows_law(outcomes: &[u64], law: &[(u64, f64)]) {
    let n = outcomes.len() as f64;
    for &v in outcomes {
        assert!(
            law.iter().any(|&(w, p)| w == v && p > 0.0),
            "value {v} has probability 0 under the law"
        );
    }
    for &(v, p) in law.iter().filter(|&&(_, p)| p > 0.0) {
        let observed = outcomes.iter().filter(|&&o| o == v).count() as f64 / n;
        let tolerance = 5.0 * (p * (1.0 - p) / n).sqrt();
        assert!(
            (observed - p).abs() <= tolerance,
            "value {v}: observed {observed:.4}, law {p:.4} (tolerance {tolerance:.4})"
        );
    }
}

#[test]
fn median_engine_adopts_the_median_of_a_uniform_ell_subset() {
    // Alg 1: of the C(6,3) = 20 subsets of [5,9,9,7,5,1] the median is 5 in 10, 7 in 6, 9 in 4
    // and never 1. A min rule, a max rule or a median over all replies (always 7) fail.
    let adopted = adopted_over_seeds(|seed| median_json(8, seed, 9, ""), &[5, 9, 9, 7, 5, 1]);
    assert_follows_law(&adopted, &[(1, 0.0), (5, 0.5), (7, 0.3), (9, 0.2)]);
}

#[test]
fn priority_engine_adopts_the_largest_of_a_uniform_ell_subset() {
    // Def 3.5: of the 20 subsets of [11,5,42,7,23,5] the maximum is 42 in 10, 23 in 6, 11 in 3,
    // 7 in 1. A max over all replies, or over the ℓ largest, always gives 42.
    let adopted = adopted_over_seeds(
        |seed| {
            format!(
                r#"{{"scenario":{{"priority":{{"n":8,"seed":{seed},"k":6,"ell":3,
                "init":{{"kind":"distinct"}},"max_rounds":9}}}},"sync":"timer"}}"#
            )
        },
        &[11, 5, 42, 7, 23, 5],
    );
    assert_follows_law(
        &adopted,
        &[(5, 0.0), (7, 0.05), (11, 0.15), (23, 0.3), (42, 0.5)],
    );
}

#[test]
fn gossip_engine_adopts_x_when_any_of_six_replies_carries_it() {
    // Alg 4 takes no ℓ-subset: a single reply carrying x among six x0 replies suffices, at
    // every position of the reply slots.
    for position in 0..6 {
        let mut replies = [0u64; 6];
        replies[position] = 5;
        let adopted = adopted_over_seeds(
            |seed| {
                format!(
                    r#"{{"scenario":{{"gossip":{{"n":8,"seed":{seed},"k":6,"ell":3,
                    "holders":2,"x":5,"x0":0,"max_rounds":9}}}},"sync":"timer"}}"#
                )
            },
            &replies,
        );
        assert!(
            adopted.iter().all(|&v| v == 5),
            "x at slot {position} must always be adopted"
        );
    }
}

#[test]
fn exactly_ell_replies_are_deterministic_per_mode() {
    // With exactly ℓ replies the chosen subset is forced — adoption is exact.
    let mut med = one_engine(&median_json(8, 2, 9, ""));
    craft_replies(&mut med, 1, &[5, 9, 9]);
    assert_eq!(
        post_value(&med.on_round_end()),
        Some(9),
        "median of 5,9,9 = 9"
    );

    let gossip_x5 = r#"{"scenario":{"gossip":{"n":8,"seed":2,"k":6,"ell":3,
        "holders":2,"x":5,"x0":0,"max_rounds":9}},"sync":"timer"}"#;
    let mut gos = one_engine(gossip_x5);
    craft_replies(&mut gos, 1, &[9, 9, 5]);
    assert_eq!(
        post_value(&gos.on_round_end()),
        Some(5),
        "the only reply carrying x adopts x"
    );
    gos.on_round_start();
    craft_replies(&mut gos, 2, &[9, 9, 7]);
    assert_eq!(
        post_value(&gos.on_round_end()),
        Some(0),
        "no x anywhere → dummy x0"
    );

    let priority = r#"{"scenario":{"priority":{"n":8,"seed":2,"k":6,"ell":3,
        "init":{"kind":"distinct"},"max_rounds":9}},"sync":"timer"}"#;
    let mut pri = one_engine(priority);
    craft_replies(&mut pri, 1, &[5, 9, 7]);
    assert_eq!(post_value(&pri.on_round_end()), Some(9), "largest of 5,9,7");
}

// --- round transitions (CP-4) ---------------------------------------------

#[test]
fn future_replies_queue_past_replies_count() {
    let mut e = one_engine(&median_json(8, 4, 9, ""));
    // round 2 reply while in round 1: queued, invisible now (slot 5 avoids
    // colliding with the crafted round-2 replies on slots 0..1)
    e.on_message(Msg::PullReply {
        round: 2,
        from: 1,
        slot: 5,
        payload: ReplyPayload::Value(7),
    });
    craft_replies(&mut e, 1, &[3, 3, 3]);
    let r1 = e.on_round_end();
    assert_eq!(r1.replies, 3, "future reply not counted in round 1");

    e.on_round_start();
    craft_replies(&mut e, 2, &[7, 7]); // 2 live + 1 queued = 3 = ℓ
    let r2 = e.on_round_end();
    assert_eq!(r2.replies, 3, "queued round-2 reply drained into its round");
    assert_eq!(post_value(&r2), Some(7));

    // stale reply for round 1 while in round 3
    e.on_round_start();
    e.on_message(Msg::PullReply {
        round: 1,
        from: 1,
        slot: 4,
        payload: ReplyPayload::Value(1),
    });
    let r3 = e.on_round_end();
    assert_eq!(r3.late_rep, 1, "past-round reply dropped + counted");
    assert_eq!(r3.replies, 0);
}

// --- spec guard specific to these modes ------------------------------------

#[test]
fn priority_arm_rejects_undecided_starts() {
    // PriorityNode has no ⊥ constructor; WithUndecided init must be rejected
    // at spec time, not panic at engine construction.
    let json = r#"{"scenario":{"priority":{"n":8,"seed":1,"k":6,"ell":3,
        "init":{"kind":"with_undecided","useful_fraction":0.5,"inner":{"kind":"distinct"}},
        "max_rounds":9}},"sync":"timer"}"#;
    assert!(NodeRunSpec::from_json(json).is_err());
}

#[test]
fn the_recorded_reply_count_is_not_truncated_for_large_k() {
    let json = r#"{"scenario":{"median":{"n":8,"seed":1,"k":300,"ell":3,
        "init":{"kind":"distinct"},"max_rounds":2}},"sync":"timer"}"#;
    let mut e = one_engine(json);
    craft_replies(&mut e, 1, &[4; 300]);
    assert_eq!(u32::from(e.on_round_end().replies), 300);
}

#[test]
fn a_frame_naming_a_nonexistent_sender_is_counted_and_dropped() {
    let mut e = one_engine(&median_json(8, 2, 9, ""));
    for round in [1, 2] {
        let out = e.on_message(Msg::PullRequest {
            round,
            from: 10_000,
            slot: 0,
            needs_state: false,
        });
        assert!(out.is_empty(), "no reply routed to a node id outside 0..n");
    }
    let rec = e.on_round_end();
    assert_eq!(rec.transport_err, 2, "malformed senders fail the gates");
    assert_eq!(rec.req_recv, 0);
    assert!(
        e.on_round_start().iter().all(|(to, _)| *to < 8),
        "the queue holds nothing addressed outside the run"
    );
}
