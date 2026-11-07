//! Feature acceptance tests — user-perspective behavior of the networked
//! node crate, written before implementation (edge-case outline:
//! .claude/plans/wip/2026-07-24-phase-6-shadow-node.edge-cases.md).
//! Each drives the public API the way the shell / collector / operator will.

use network_node::collect::aggregate;
use network_node::engine::RoundEngine;
use network_node::spec::NodeRunSpec;
use network_node::wire::{AckKind, Msg, ReplyPayload, decode, encode};

fn median_spec_json(n: usize, seed: u64, max_rounds: usize) -> String {
    format!(
        r#"{{"scenario":{{"median":{{"n":{n},"seed":{seed},"k":6,"ell":3,
             "init":{{"kind":"distinct"}},"max_rounds":{max_rounds}}}}},
             "sync":"timer"}}"#
    )
}

/// Perfect in-memory network: run all engines to the horizon, deliver every
/// emitted message instantly, return per-node records.
fn run_harness(spec: &NodeRunSpec) -> Vec<Vec<network_node::record::RoundRecord>> {
    network_node::harness::run(spec)
}

// --- spec parsing / validation -------------------------------------------

#[test]
fn spec_parses_all_five_modes_and_rejects_invalid() {
    // median
    let s = NodeRunSpec::from_json(&median_spec_json(16, 1, 50)).expect("median spec");
    assert_eq!(s.n(), 16);
    assert_eq!(s.round_ms, 200, "round_ms defaults to 200 (CP-4)");

    // extended + compact via the sim SMR family
    for proto in [
        r#"{"kind":"extended"}"#,
        r#"{"kind":"compact","t_commit_rounds":8}"#,
    ] {
        let json = format!(
            r#"{{"scenario":{{"smr":{{"n":16,"seed":1,"k":6,"ell":3,"sigma":1.0,
                 "proto":{proto},"injections":[],"max_rounds":40,
                 "schedule":{{"kind":"fresh_per_round","fraction":0.0}}}}}},
                 "sync":"timer"}}"#
        );
        NodeRunSpec::from_json(&json).expect("smr spec");
    }

    // gossip + priority arms
    let gossip = r#"{"scenario":{"gossip":{"n":16,"seed":1,"k":6,"ell":3,
        "holders":4,"x":7,"x0":0,"max_rounds":40}},"sync":"timer"}"#;
    NodeRunSpec::from_json(gossip).expect("gossip spec");
    let priority = r#"{"scenario":{"priority":{"n":16,"seed":1,"k":6,"ell":3,
        "init":{"kind":"distinct"},"max_rounds":40}},"sync":"timer"}"#;
    NodeRunSpec::from_json(priority).expect("priority spec");

    // even ell rejected (Config validation must be reached)
    let bad = median_spec_json(16, 1, 50).replace(r#""ell":3"#, r#""ell":4"#);
    assert!(
        NodeRunSpec::from_json(&bad).is_err(),
        "even ell must be rejected"
    );

    // unknown field rejected (deny_unknown_fields parity with sim::spec)
    let unknown =
        median_spec_json(16, 1, 50).replace(r#""sync":"timer""#, r#""sync":"timer","bogus":1"#);
    assert!(
        NodeRunSpec::from_json(&unknown).is_err(),
        "unknown field must be rejected"
    );
}

// --- wire framing ---------------------------------------------------------

#[test]
fn wire_roundtrips_every_message_kind() {
    let max = 64 * 1024 * 1024;
    let msgs = vec![
        Msg::PullRequest {
            round: 3,
            from: 1,
            slot: 5,
            needs_state: true,
        },
        Msg::PullReply {
            round: 3,
            from: 2,
            slot: 5,
            payload: ReplyPayload::Value(42),
        },
        Msg::PullReply {
            round: 4,
            from: 0,
            slot: 0,
            payload: ReplyPayload::Log(vec![1, 2, 3]),
        },
        Msg::Append {
            round: 9,
            from: 7,
            client: 5,
            sn: 1,
            op: 100,
        },
        Msg::ClientCmd {
            round: 9,
            client: 5,
            sn: 2,
            op: 101,
        },
        Msg::ClientAck {
            round: 9,
            client: 5,
            sn: 2,
            kind: AckKind::Amplified,
        },
        Msg::RoundDone { round: 12, from: 3 },
        Msg::RoundGo { round: 13 },
    ];
    for m in msgs {
        let frame = encode(&m, max, 0).expect("encode");
        let back = decode(&frame, max).expect("decode");
        assert_eq!(back, m);
    }
    // empty-but-present log is distinct from silence: it still encodes
    let empty_log = Msg::PullReply {
        round: 1,
        from: 0,
        slot: 0,
        payload: ReplyPayload::Log(vec![]),
    };
    let frame = encode(&empty_log, max, 0).expect("empty log encodes");
    assert_eq!(decode(&frame, max).expect("decodes"), empty_log);
}

#[test]
fn wire_rejects_frames_over_the_cap() {
    let big = Msg::PullReply {
        round: 1,
        from: 0,
        slot: 0,
        payload: ReplyPayload::Log(vec![0u64; 1_000_000]),
    };
    // 1M u64 entries ≈ 8MB body: must encode under a 64MiB cap…
    assert!(encode(&big, 64 * 1024 * 1024, 0).is_ok());
    // …and be rejected under a 1KiB cap — deterministically, not by panic.
    assert!(encode(&big, 1024, 0).is_err());
}

// --- median convergence through the harness -------------------------------

#[test]
fn median_population_converges_to_one_value_and_stays_there() {
    let spec = NodeRunSpec::from_json(&median_spec_json(16, 7, 60)).expect("spec");
    let records = run_harness(&spec);
    let last: Vec<Option<u64>> = records
        .iter()
        .map(|r| match r.last().expect("rounds ran").post {
            network_node::record::PostState::Value(v) => v,
            _ => panic!("median mode emits value posts"),
        })
        .collect();
    let held: Vec<u64> = last.iter().filter_map(|v| *v).collect();
    assert!(
        !held.is_empty(),
        "population must not die under zero blocking"
    );
    let first = held[0];
    assert!(
        held.iter().all(|v| *v == first),
        "all holders agree at horizon: {last:?}"
    );
}

#[test]
fn collect_reports_persistent_convergence_and_sim_schema_csv() {
    let spec = NodeRunSpec::from_json(&median_spec_json(16, 7, 60)).expect("spec");
    let records = run_harness(&spec);
    let out = aggregate(&records, &spec).expect("aggregate");
    assert!(
        out.metrics_csv
            .starts_with("round,holders,undecided,blocked,useful,distinct_values"),
        "single-value CSV header must match crates/sim/src/runlog.rs"
    );
    let conv = out.summary.convergence_round.expect("converged run");
    assert!((1..=60).contains(&conv));
}

// --- SMR ack terminality (F16) --------------------------------------------

fn smr_spec_json(proto: &str) -> String {
    format!(
        r#"{{"scenario":{{"smr":{{"n":8,"seed":3,"k":6,"ell":3,"sigma":1.0,
             "proto":{proto},"injections":[],"max_rounds":60,
             "schedule":{{"kind":"fresh_per_round","fraction":0.0}}}}}},
             "sync":"timer"}}"#
    )
}

/// Drive one client against a live engine population by hand: send the
/// command to node 0 each round until the ack kind satisfies `terminal`.
fn acks_until_terminal(proto: &str, terminal: AckKind) -> Vec<AckKind> {
    let spec = NodeRunSpec::from_json(&smr_spec_json(proto)).expect("spec");
    let n = spec.n();
    let mut engines: Vec<RoundEngine> = (0..n as u32)
        .map(|id| RoundEngine::new(&spec, id).expect("engine"))
        .collect();
    let mut acks = Vec::new();
    for round in 1..=spec.max_rounds() as u64 {
        let mut in_flight: Vec<(u32, Msg)> = Vec::new();
        for e in engines.iter_mut() {
            in_flight.extend(e.on_round_start());
        }
        // client stage: resend until terminal ack
        if acks.last() != Some(&terminal) {
            in_flight.push((
                0,
                Msg::ClientCmd {
                    round,
                    client: 1,
                    sn: 1,
                    op: 500,
                },
            ));
        }
        while let Some((to, m)) = in_flight.pop() {
            if let Msg::ClientAck { kind, .. } = m {
                acks.push(kind);
                continue;
            }
            in_flight.extend(engines[to as usize].on_message(m));
        }
        for e in engines.iter_mut() {
            e.on_round_end();
        }
        if acks.last() == Some(&terminal) {
            break;
        }
    }
    acks
}

#[test]
fn extended_delivered_is_terminal() {
    let acks = acks_until_terminal(r#"{"kind":"extended"}"#, AckKind::Delivered);
    assert_eq!(acks.last(), Some(&AckKind::Delivered), "acks: {acks:?}");
}

#[test]
fn compact_resends_until_ack_committed() {
    let acks = acks_until_terminal(
        r#"{"kind":"compact","t_commit_rounds":5}"#,
        AckKind::AckCommitted,
    );
    assert_eq!(acks.last(), Some(&AckKind::AckCommitted), "acks: {acks:?}");
    assert!(
        acks.contains(&AckKind::Amplified),
        "first accepted delivery triages Amplify — non-terminal (F16): {acks:?}"
    );
    assert!(
        acks.iter().filter(|k| **k == AckKind::Ignored).count() >= 1,
        "resends of an in-log uncommitted command are Ignored, and resend continues: {acks:?}"
    );
}

// --- blocking makes a node silent + ⊥ -------------------------------------

#[test]
fn permanently_blocked_node_ends_every_round_undecided() {
    // Permanent 25% blocking: blocked nodes must end each round ⊥ and never serve.
    let json = r#"{"scenario":{"median":{"n":8,"seed":11,"k":6,"ell":3,
        "init":{"kind":"distinct"},"max_rounds":30,
        "schedule":{"kind":"permanent","fraction":0.25}}},"sync":"timer"}"#;
    let spec = NodeRunSpec::from_json(json).expect("spec");
    let records = run_harness(&spec);
    let mut saw_blocked = false;
    for per_node in &records {
        for rec in per_node {
            if rec.blocked {
                saw_blocked = true;
                assert_eq!(rec.req_sent, 0, "blocked node sends no pulls");
                assert_eq!(rec.rep_sent, 0, "blocked node serves nothing");
                assert_eq!(
                    rec.post,
                    network_node::record::PostState::Value(None),
                    "blocked round ends ⊥ (round {})",
                    rec.r
                );
            }
        }
    }
    assert!(saw_blocked, "permanent 25% schedule must block someone");
}
