//! A node's outputs are a function of (seed, config) and the multiset of messages per round,
//! never of the order the transport delivers them in.

use network_node::engine::RoundEngine;
use network_node::record::{PostState, RoundRecord};
use network_node::spec::NodeRunSpec;
use network_node::wire::{Msg, ReplyPayload};
use protocol::compact::{Entry, Timed};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;

const EXT: &str = r#"{"kind":"extended"}"#;
const COMP: &str = r#"{"kind":"compact","t_commit_rounds":3}"#;

fn spec(n: usize, sigma: f64, proto: &str, rounds: usize, tracked: &str) -> NodeRunSpec {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"smr":{{"n":{n},"seed":3,"k":6,"ell":3,"sigma":{sigma},
            "proto":{proto},"injections":[],"max_rounds":{rounds},
            "schedule":{{"kind":"fresh_per_round","fraction":0.0}}}}}},
            "sync":"timer"{tracked}}}"#
    ))
    .expect("spec")
}

fn append(from: u32, client: u32, op: u64) -> Msg {
    Msg::Append {
        round: 1,
        from,
        client,
        sn: 1,
        op,
    }
}

/// Fixed-seed shuffled delivery of everything in flight, commands included.
fn run_shuffled(spec: &NodeRunSpec, order_seed: u64) -> Vec<Vec<RoundRecord>> {
    let n = spec.n();
    let mut engines: Vec<RoundEngine> = (0..n as u32)
        .map(|id| RoundEngine::new(spec, id).expect("engine"))
        .collect();
    let mut records: Vec<Vec<RoundRecord>> = vec![Vec::new(); n];
    let mut order = ChaCha12Rng::seed_from_u64(order_seed);
    for round in 1..=spec.max_rounds() as u64 {
        let mut in_flight: Vec<(u32, Msg)> = Vec::new();
        for e in engines.iter_mut() {
            in_flight.extend(e.on_round_start());
        }
        if round <= 3 {
            for c in 0..4u32 {
                in_flight.push((
                    (c * 3 + round as u32) % n as u32,
                    Msg::ClientCmd {
                        round,
                        client: c + 1,
                        sn: round,
                        op: 100 * round + c as u64,
                    },
                ));
            }
        }
        while !in_flight.is_empty() {
            let (to, m) = in_flight.swap_remove(order.random_range(0..in_flight.len()));
            if matches!(m, Msg::ClientAck { .. }) {
                continue;
            }
            in_flight.extend(engines[to as usize].on_message(m));
        }
        for (id, e) in engines.iter_mut().enumerate() {
            records[id].push(e.on_round_end());
        }
    }
    records
}

type Fingerprint = (PostState, u32, u32, u32, u32);

fn deterministic_fields(records: &[Vec<RoundRecord>]) -> Vec<Vec<Fingerprint>> {
    records
        .iter()
        .map(|rs| {
            rs.iter()
                .map(|r| {
                    (
                        r.post.clone(),
                        r.app_sent,
                        r.app_recv,
                        r.acks,
                        r.replies as u32,
                    )
                })
                .collect()
        })
        .collect()
}

#[test]
fn extended_run_does_not_depend_on_delivery_order() {
    let spec = spec(16, 2.0, EXT, 12, "");
    let reference = deterministic_fields(&run_shuffled(&spec, 0));
    for order_seed in 1..6 {
        assert_eq!(
            reference,
            deterministic_fields(&run_shuffled(&spec, order_seed)),
            "delivery order {order_seed} changed the run"
        );
    }
}

#[test]
fn compact_run_does_not_depend_on_delivery_order() {
    let spec = spec(16, 2.0, COMP, 12, "");
    let reference = deterministic_fields(&run_shuffled(&spec, 0));
    for order_seed in 1..6 {
        assert_eq!(
            reference,
            deterministic_fields(&run_shuffled(&spec, order_seed)),
            "delivery order {order_seed} changed the run"
        );
    }
}

fn extended_positions(first: Msg, second: Msg) -> (Option<usize>, Option<usize>) {
    let spec = spec(8, 1.0, EXT, 4, r#","tracked_ops":[100,200]"#);
    let mut e = RoundEngine::new(&spec, 7).expect("engine");
    e.on_round_start();
    for slot in 0..3u16 {
        e.on_message(Msg::PullReply {
            round: 1,
            from: 1,
            slot,
            payload: ReplyPayload::Log(vec![sim::smr::SEED_COMMAND]),
        });
    }
    e.on_message(first);
    e.on_message(second);
    let rec = e.on_round_end();
    (rec.tracked[0].pos, rec.tracked[1].pos)
}

#[test]
fn appends_enter_the_log_in_sender_id_order_whatever_the_arrival_order() {
    let from3 = append(3, 1, 100);
    let from4 = append(4, 2, 200);
    let want = (Some(1), Some(2));
    assert_eq!(extended_positions(from3.clone(), from4.clone()), want);
    assert_eq!(extended_positions(from4, from3), want);
}

#[test]
fn compact_appends_enter_the_log_in_sender_id_order_whatever_the_arrival_order() {
    let positions = |swap: bool| {
        let spec = spec(8, 1.0, COMP, 4, r#","tracked_ops":[100,200]"#);
        let mut e = RoundEngine::new(&spec, 7).expect("engine");
        e.on_round_start();
        for slot in 0..3u16 {
            e.on_message(Msg::PullReply {
                round: 1,
                from: 1,
                slot,
                payload: ReplyPayload::Compact {
                    log: vec![Timed {
                        entry: Entry::Nop(1),
                        round: 0,
                    }],
                    state: None,
                },
            });
        }
        let (x, y) = (append(3, 1, 100), append(4, 2, 200));
        let (first, second) = if swap { (y, x) } else { (x, y) };
        e.on_message(first);
        e.on_message(second);
        let rec = e.on_round_end();
        (rec.tracked[0].pos, rec.tracked[1].pos)
    };
    assert_eq!(positions(false), positions(true));
    let (a, b) = positions(false);
    assert!(a < b, "sender 3 precedes sender 4: {a:?} {b:?}");
}

fn amplify_targets(msgs: Vec<Msg>, watch: u32) -> Vec<u32> {
    let spec = spec(16, 2.0, EXT, 4, "");
    let mut e = RoundEngine::new(&spec, 0).expect("engine");
    e.on_round_start();
    let mut targets = Vec::new();
    for msg in msgs {
        for (to, out) in e.on_message(msg) {
            if matches!(out, Msg::Append { client, .. } if client == watch) {
                targets.push(to);
            }
        }
    }
    targets
}

fn cmd(round: u64, client: u32) -> Msg {
    Msg::ClientCmd {
        round,
        client,
        sn: 1,
        op: 10 * client as u64,
    }
}

#[test]
fn amplify_targets_depend_only_on_the_command_not_on_its_neighbours() {
    let alone = amplify_targets(vec![cmd(1, 2)], 2);
    assert!(!alone.is_empty(), "precondition: the command amplified");
    let after = amplify_targets(vec![cmd(1, 1), cmd(1, 2)], 2);
    let before = amplify_targets(vec![cmd(1, 2), cmd(1, 1)], 2);
    assert_eq!(alone, after);
    assert_eq!(alone, before);
}

#[test]
fn amplify_targets_differ_between_commands_and_rounds() {
    let first = amplify_targets(vec![cmd(1, 2)], 2);
    let other_client = amplify_targets(vec![cmd(1, 3)], 3);
    assert_ne!(first, other_client, "independent draws per command");
}

#[test]
fn a_client_command_does_not_shift_later_pull_targets() {
    let targets = |with_cmd: bool| {
        let spec = spec(16, 2.0, EXT, 4, "");
        let mut e = RoundEngine::new(&spec, 0).expect("engine");
        e.on_round_start();
        if with_cmd {
            e.on_message(cmd(1, 1));
        }
        e.on_round_end();
        e.on_round_start()
            .into_iter()
            .map(|(to, _)| to)
            .collect::<Vec<_>>()
    };
    assert_eq!(targets(false), targets(true));
}

#[test]
fn a_late_client_command_is_counted_and_not_processed() {
    let spec = spec(8, 1.0, EXT, 4, "");
    let mut e = RoundEngine::new(&spec, 0).expect("engine");
    e.on_round_start();
    e.on_round_end();
    e.on_round_start();
    let out = e.on_message(cmd(1, 1));
    assert!(
        out.is_empty(),
        "a round-1 command in round 2 gets no ack and no append"
    );
    let rec = e.on_round_end();
    assert_eq!(rec.late_cli, 1);
    assert_eq!(rec.cli_recv, 0);
}

#[test]
fn an_early_client_command_is_deferred_not_processed() {
    let spec = spec(8, 1.0, EXT, 4, "");
    let mut e = RoundEngine::new(&spec, 0).expect("engine");
    e.on_round_start();
    let early = cmd(2, 1);
    assert!(
        e.defers(&early),
        "a command tagged for the next round waits"
    );
    assert!(!e.defers(&cmd(1, 1)));
    assert!(e.on_message(early).is_empty());
    assert_eq!(e.on_round_end().cli_recv, 0);
}

#[test]
fn a_client_command_two_rounds_ahead_is_counted_as_early_not_as_a_past_drop() {
    let spec = spec(8, 1.0, EXT, 4, "");
    let mut e = RoundEngine::new(&spec, 0).expect("engine");
    e.on_round_start();
    let far = cmd(3, 1);
    assert!(!e.defers(&far), "only the next round is held back");
    assert!(e.on_message(far).is_empty());
    let rec = e.on_round_end();
    assert_eq!((rec.early_cli, rec.dropped_past, rec.cli_recv), (1, 0, 0));
}
