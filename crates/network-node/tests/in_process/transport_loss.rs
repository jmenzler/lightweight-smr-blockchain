//! Message loss outside blocking must be visible to the validity gates; blocking must not trip them.

use network_node::collect::{aggregate, aggregate_smr};
use network_node::engine::RoundEngine;
use network_node::harness;
use network_node::record::RoundRecord;
use network_node::spec::NodeRunSpec;
use network_node::wire::Msg;

fn gossip_spec(extra_schedule: &str) -> NodeRunSpec {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"gossip":{{"n":16,"seed":5,"k":6,"ell":3,"holders":4,
            "x":7,"x0":0,"max_rounds":6{extra_schedule}}}}},"sync":"timer"}}"#
    ))
    .expect("spec")
}

/// Perfect network except that `drop` may veto any in-flight message.
fn run_with_loss(
    spec: &NodeRunSpec,
    mut drop: impl FnMut(u32, &Msg) -> bool,
) -> Vec<Vec<RoundRecord>> {
    let n = spec.n();
    let mut engines: Vec<RoundEngine> = (0..n as u32)
        .map(|id| RoundEngine::new(spec, id).expect("engine"))
        .collect();
    let mut records: Vec<Vec<RoundRecord>> = vec![Vec::new(); n];
    for _ in 0..spec.max_rounds() {
        let mut in_flight: Vec<(u32, Msg)> = Vec::new();
        for e in engines.iter_mut() {
            in_flight.extend(e.on_round_start());
        }
        while let Some((to, m)) = in_flight.pop() {
            if drop(to, &m) {
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

#[test]
fn a_lossless_unblocked_run_is_gates_clean() {
    let spec = gossip_spec("");
    let out = aggregate(&harness::run(&spec), &spec).expect("aggregate");
    assert!(out.gates.clean(), "{:?}", out.gates);
}

#[test]
fn blocking_alone_never_trips_the_reconciliation() {
    let spec = gossip_spec(r#","schedule":{"kind":"fresh_per_round","fraction":0.4}"#);
    let records = harness::run(&spec);
    assert!(records.iter().flatten().any(|r| r.blocked), "precondition");
    let out = aggregate(&records, &spec).expect("aggregate");
    assert!(out.gates.clean(), "{:?}", out.gates);
}

#[test]
fn silently_dropped_replies_are_not_gates_clean() {
    let spec = gossip_spec("");
    let mut dropped = 0;
    let records = run_with_loss(&spec, |to, m| {
        let hit = to == 0 && matches!(m, Msg::PullReply { .. });
        dropped += u32::from(hit);
        hit
    });
    assert!(dropped > 0, "precondition");
    let out = aggregate(&records, &spec).expect("aggregate");
    assert_eq!(u64::from(dropped), out.gates.rep_unreconciled);
    assert!(!out.gates.clean());
}

#[test]
fn silently_dropped_requests_are_not_gates_clean() {
    let spec = gossip_spec("");
    let records = run_with_loss(&spec, |to, m| {
        to == 3 && matches!(m, Msg::PullRequest { .. })
    });
    let out = aggregate(&records, &spec).expect("aggregate");
    assert!(out.gates.req_unreconciled > 0, "{:?}", out.gates);
    assert!(!out.gates.clean());
}

fn smr_spec(schedule: &str) -> NodeRunSpec {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"smr":{{"n":8,"seed":3,"k":6,"ell":3,"sigma":2.0,
            "proto":{{"kind":"extended"}},"injections":[],"max_rounds":6,
            "schedule":{schedule}}}}},"sync":"timer"}}"#
    ))
    .expect("spec")
}

fn run_extended_with_commands(
    spec: &NodeRunSpec,
    mut drop: impl FnMut(&Msg) -> bool,
) -> Vec<Vec<RoundRecord>> {
    let n = spec.n();
    let mut engines: Vec<RoundEngine> = (0..n as u32)
        .map(|id| RoundEngine::new(spec, id).expect("engine"))
        .collect();
    let mut records: Vec<Vec<RoundRecord>> = vec![Vec::new(); n];
    for round in 1..=spec.max_rounds() as u64 {
        let mut in_flight: Vec<(u32, Msg)> = Vec::new();
        for e in engines.iter_mut() {
            in_flight.extend(e.on_round_start());
        }
        for id in 0..n as u32 {
            in_flight.extend(engines[id as usize].on_message(Msg::ClientCmd {
                round,
                client: id + 1,
                sn: round,
                op: 1000 * round + id as u64,
            }));
        }
        while let Some((to, m)) = in_flight.pop() {
            if matches!(m, Msg::ClientAck { .. }) || drop(&m) {
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

#[test]
fn appends_to_blocked_servers_reconcile_but_dropped_appends_do_not() {
    let spec = smr_spec(r#"{"kind":"fresh_per_round","fraction":0.4}"#);
    let clean = run_extended_with_commands(&spec, |_| false);
    assert!(
        clean.iter().flatten().any(|r| r.app_blocked > 0),
        "precondition"
    );
    let out = aggregate_smr(&clean, &spec, None).expect("aggregate");
    assert_eq!(out.gates.app_unreconciled, 0, "{:?}", out.gates);

    let lossy = run_extended_with_commands(&spec, |m| matches!(m, Msg::Append { .. }));
    let out = aggregate_smr(&lossy, &spec, None).expect("aggregate");
    assert!(out.gates.app_unreconciled > 0, "{:?}", out.gates);
    assert!(!out.gates.clean());
}
