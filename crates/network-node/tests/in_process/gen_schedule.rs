//! `gen_schedule`'s pure generator: golden JSON for a fixed seed, wire
//! round-trip through `sim::spec::InjectionSpec`, and the per-client rank
//! sanity check via `sim::smr::injection_sns`.

use network_node::schedule::generate;
use sim::smr::{Injection, injection_sns};
use sim::spec::InjectionSpec;
use std::collections::HashMap;

#[test]
fn golden_seed_42_n3_two_clients() {
    let schedule = generate(42, 10, &[0.5, 0.5], 2, 2, 3, true).expect("generate");
    let json = serde_json::to_string(&schedule).expect("serialize");
    assert_eq!(
        json,
        r#"[{"round":1,"client":1,"op":1,"target":0},{"round":4,"client":2,"op":2,"target":0},{"round":5,"client":1,"op":3,"target":2},{"round":6,"client":2,"op":4,"target":0}]"#
    );
}

#[test]
fn output_deserializes_as_injection_spec_vec() {
    let schedule = generate(7, 30, &[0.2, 0.3, 0.5], 3, 4, 5, true).expect("generate");
    let json = serde_json::to_string(&schedule).expect("serialize");
    let parsed: Vec<InjectionSpec> = serde_json::from_str(&json).expect("wire-compatible");
    assert_eq!(parsed.len(), schedule.len());
    for (a, b) in schedule.iter().zip(&parsed) {
        assert_eq!(a.round, b.round);
        assert_eq!(a.client, b.client);
        assert_eq!(a.op, b.op);
        assert_eq!(a.target, b.target);
        assert!(b.target.is_some(), "pinned schedules carry a target");
    }
}

#[test]
fn every_client_gets_ranks_one_through_per_client() {
    let clients = 4u32;
    let per_client = 5u32;
    let schedule =
        generate(99, 200, &[0.4, 0.4, 0.2], clients, per_client, 6, true).expect("generate");
    assert_eq!(schedule.len(), (clients * per_client) as usize);

    let injections: Vec<Injection> = schedule
        .iter()
        .map(|i| Injection {
            round: i.round,
            client: i.client,
            op: i.op,
            target: i.target,
        })
        .collect();
    let sns = injection_sns(&injections);

    let mut by_client: HashMap<u32, Vec<(usize, u64)>> = HashMap::new();
    for (inj, &sn) in injections.iter().zip(&sns) {
        by_client
            .entry(inj.client)
            .or_default()
            .push((inj.round, sn));
    }
    assert_eq!(by_client.len(), clients as usize);
    for (client, mut ranks) in by_client {
        ranks.sort_by_key(|&(round, _)| round);
        let sns: Vec<u64> = ranks.iter().map(|&(_, sn)| sn).collect();
        assert_eq!(
            sns,
            (1..=per_client as u64).collect::<Vec<_>>(),
            "client {client} ranks"
        );
    }
}

#[test]
fn rejects_a_horizon_too_short_to_fit_every_command() {
    // pmf is point-mass-0: no round ever draws an arrival.
    assert!(
        generate(1, 5, &[1.0], 2, 3, 4, true).is_err(),
        "zero arrivals forever can never place 6 commands"
    );
}

#[test]
fn rejects_targets_and_ops_that_would_reach_the_auto_arrival_namespace() {
    assert!(
        generate(1, 10, &[0.0, 1.0], sim::smr::AUTO_CLIENT_BASE, 1, 4, true).is_err(),
        "clients must stay below the auto-arrival client base"
    );
    assert!(
        generate(1, 10, &[0.0, 1.0], 1, 0, 0, true).is_err(),
        "n must be at least 1 for a target draw"
    );
}

#[test]
fn unpinned_schedules_leave_every_attempt_a_fresh_uniform_draw() {
    let pinned = generate(42, 10, &[0.5, 0.5], 2, 2, 3, true).expect("generate");
    let free = generate(42, 10, &[0.5, 0.5], 2, 2, 3, false).expect("generate");
    assert!(free.iter().all(|i| i.target.is_none()));
    let strip = |v: &[sim::spec::InjectionSpec]| {
        v.iter()
            .map(|i| (i.round, i.client, i.op))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        strip(&pinned),
        strip(&free),
        "pinning must not move the arrivals"
    );
    let json = serde_json::to_string(&free).expect("serialize");
    assert!(!json.contains("target"), "an unpinned spec omits the field");
}

// p. 25: a client retries a server picked at random each time, so permanent blocking cannot strand a command.
#[test]
fn unpinned_commands_still_commit_under_permanent_blocking() {
    let injections = generate(1, 12, &[0.0, 0.0, 1.0], 20, 1, 20, false).expect("generate");
    let spec = serde_json::json!({
        "n": 20, "seed": 1, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "compact", "t_commit_rounds": 12},
        "injections": injections,
        "max_rounds": 80,
        "schedule": {"kind": "permanent", "fraction": 0.2},
    });
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(spec).expect("spec");
    let report = sim::smr::run_smr(&sim::smr::SmrScenario::try_from(spec).expect("scenario"));
    let stranded: Vec<u64> = report
        .commands
        .iter()
        .filter(|c| c.committed_ack_round.is_none())
        .map(|c| c.op)
        .collect();
    assert!(stranded.is_empty(), "never committed: {stranded:?}");
}
