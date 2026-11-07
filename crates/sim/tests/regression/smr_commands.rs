//! Golden shape test for the sim-side commands.json export — must match the
//! node collector's shape exactly (crates/network-node/src/bin/collect.rs) so
//! analysis/e7_arm2b.py can load either side with one loader.

use serde_json::Value;
use sim::smr_commands::commands_json;
use sim::spec::SmrScenarioSpec;
use std::collections::BTreeSet;

fn run(json: &str) -> Value {
    let spec: SmrScenarioSpec = serde_json::from_str(json).expect("spec parses");
    let out = commands_json(spec).expect("scenario runs");
    serde_json::from_str(&out).expect("output is valid json")
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

fn failing_spec() -> SmrScenarioSpec {
    serde_json::from_str(
        r#"{
            "n": 598, "seed": 1, "k": 6, "ell": 3, "sigma": 1.0,
            "proto": {"kind": "recovery", "t_window_rounds": 20, "resend_until_acked": true},
            "injections": [
                {"round": 2, "client": 1, "op": 1},
                {"round": 2, "client": 2, "op": 2}
            ],
            "max_rounds": 200,
            "schedule": {"kind": "fresh_per_round", "fraction": 0.1},
            "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 1.0]},
            "certs": true
        }"#,
    )
    .expect("failure fixture parses")
}

#[test]
fn failed_run_is_rejected_before_commands_json_is_constructed() {
    let error = commands_json(failing_spec()).expect_err("failed run is not scientific output");
    for field in [
        "SMR failed during BoundaryPreflight",
        "attempted round",
        "completed round",
        "observed round",
    ] {
        assert!(error.contains(field), "missing {field:?}: {error}");
    }
}

#[test]
fn extended_scenario_exports_node_shape() {
    let out = run(r#"{
        "n": 8, "seed": 3, "k": 6, "ell": 3, "sigma": 5.0,
        "proto": {"kind": "extended"},
        "injections": [
            {"round": 1, "client": 1, "op": 7},
            {"round": 1, "client": 2, "op": 9}
        ],
        "max_rounds": 30
    }"#);

    assert_eq!(
        keys(&out),
        BTreeSet::from(["client".to_string(), "landmarks".to_string()]),
        "top-level shape must match the node collector exactly"
    );

    let commands = out["client"]["commands"]
        .as_array()
        .expect("commands array");
    assert_eq!(commands.len(), 2);
    for cmd in commands {
        assert_eq!(
            keys(cmd),
            BTreeSet::from(
                [
                    "client",
                    "sn",
                    "op",
                    "injection_round",
                    "delivered_round",
                    "committed_ack_round",
                    "terminal"
                ]
                .map(String::from)
            ),
            "command field names must match the node collector's golden shape"
        );
        assert!(
            cmd["delivered_round"].is_u64(),
            "beta=0 delivers on the injection round: {cmd:?}"
        );
    }

    let landmarks = out["landmarks"].as_array().expect("landmarks array");
    assert_eq!(
        landmarks.len(),
        2,
        "landmarks cover every manual-injection op"
    );
    let ops: BTreeSet<u64> = landmarks
        .iter()
        .map(|l| l["op"].as_u64().expect("op"))
        .collect();
    assert_eq!(ops, BTreeSet::from([7, 9]));
    for lm in landmarks {
        assert_eq!(
            keys(lm),
            BTreeSet::from(["op", "all_logs_round", "prefix_fixed_round"].map(String::from)),
            "landmark field names must match the node collector's golden shape"
        );
    }
}

#[test]
fn compact_scenario_reaches_terminal_commit() {
    let out = run(r#"{
        "n": 32, "seed": 9, "k": 6, "ell": 3, "sigma": 5.0,
        "proto": {"kind": "compact", "t_commit_rounds": 6},
        "injections": [{"round": 2, "client": 1, "op": 7}],
        "max_rounds": 40
    }"#);

    let commands = out["client"]["commands"]
        .as_array()
        .expect("commands array");
    assert_eq!(commands.len(), 1);
    let cmd = &commands[0];
    assert!(
        cmd["committed_ack_round"].is_u64(),
        "compact scenario ages past t_commit_rounds: {cmd:?}"
    );
    assert_eq!(
        cmd["terminal"],
        Value::Bool(true),
        "compact terminal = committed_ack_round reached"
    );
}
