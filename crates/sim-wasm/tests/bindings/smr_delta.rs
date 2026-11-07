//! `report_delta_json` pins: the deltas, applied step by step to an empty report, rebuild
//! `report_json()` exactly (Value equality, so an absent field differs from `null`).

use serde_json::{Value, json};
use sim_wasm::LiveSmr;

const PROTOS: [&str; 4] = [
    r#"{"kind":"extended"}"#,
    r#"{"kind":"compact","t_commit_rounds":6}"#,
    r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}"#,
    r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":true}"#,
];

fn spec(proto: &str, seed: u64, injections: &str, traffic: Option<&str>) -> String {
    let traffic = traffic
        .map(|pmf| format!(r#","traffic":{{"kind":"pmf","arrivals_pmf":{pmf}}}"#))
        .unwrap_or_default();
    format!(
        r#"{{"n":32,"seed":{seed},"k":6,"ell":3,"sigma":1.5,"proto":{proto},
            "injections":{injections},"max_rounds":400,
            "schedule":{{"kind":"fresh_per_round","fraction":0.1}}{traffic}}}"#
    )
}

const ONE_INJECTION: &str = r#"[{"round":2,"client":1,"op":7}]"#;

fn parse(json: &str) -> Value {
    serde_json::from_str(json).expect("json")
}

/// The merge the page performs, restated over `Value`s.
fn apply(prev: &Value, delta: &Value) -> Value {
    let len = |v: &Value| v.as_array().map_or(0, Vec::len) as u64;
    assert_eq!(
        delta["rounds_from"],
        len(&prev["metrics"]),
        "metrics cursor"
    );
    let prev_rows = prev["recovery"]["rounds"].clone();
    assert_eq!(delta["rec_rows_from"], len(&prev_rows), "recovery cursor");

    let mut old = prev["commands"].as_array().unwrap().iter();
    let inserted = delta["inserted"].as_array().unwrap();
    let mut next_insert = inserted.iter().peekable();
    let total = delta["len"].as_u64().unwrap() as usize;
    let mut commands = Vec::with_capacity(total);
    for i in 0..total {
        match next_insert.peek() {
            Some(pair) if pair[0] == i => commands.push(next_insert.next().unwrap()[1].clone()),
            _ => commands.push(old.next().expect("old command").clone()),
        }
    }
    assert!(old.next().is_none() && next_insert.next().is_none());
    for pair in delta["changed"].as_array().unwrap() {
        commands[pair[0].as_u64().unwrap() as usize] = pair[1].clone();
    }
    for row in delta["appended"].as_array().unwrap() {
        let at = row[0].as_u64().unwrap() as usize;
        let mut command = row[1].clone();
        for (field, from, tail) in [("spread", &row[2], &row[3]), ("attempts", &row[4], &row[5])] {
            let old = commands[at][field].as_array().expect("array to append to");
            assert_eq!(*from, old.len(), "{field} cursor");
            let mut grown = old.clone();
            grown.extend(tail.as_array().unwrap().iter().cloned());
            command[field] = Value::Array(grown);
        }
        commands[at] = command;
    }

    let concat = |a: &Value, b: &Value| {
        let mut rows = a.as_array().cloned().unwrap_or_default();
        rows.extend(b.as_array().unwrap().iter().cloned());
        Value::Array(rows)
    };
    let mut report = delta["summary"].clone();
    report["metrics"] = concat(&prev["metrics"], &delta["metrics"]);
    report["commands"] = Value::Array(commands);
    if report.get("recovery").is_some() {
        report["recovery"]["rounds"] = concat(&prev_rows, &delta["rec_rows"]);
    }
    report
}

struct Mirror {
    merged: Value,
    syncs: usize,
}

impl Mirror {
    fn new() -> Self {
        Mirror {
            merged: json!({ "metrics": [], "commands": [] }),
            syncs: 0,
        }
    }

    fn sync(&mut self, live: &mut LiveSmr, context: &str) {
        let delta = parse(&live.report_delta_json());
        self.merged = apply(&self.merged, &delta);
        self.syncs += 1;
        assert_eq!(
            self.merged,
            parse(&live.report_json()),
            "{context}: sync {}",
            self.syncs
        );
    }
}

fn run(spec_json: &str, steps: usize, context: &str) {
    let mut live = LiveSmr::new(spec_json).expect("session");
    let mut mirror = Mirror::new();
    mirror.sync(&mut live, context);
    for _ in 0..steps {
        live.step(0.1);
        mirror.sync(&mut live, context);
    }
}

#[test]
fn every_step_delta_rebuilds_the_full_report() {
    for proto in PROTOS {
        for seed in [1, 2, 3] {
            for traffic in [None, Some("[0.3,0.4,0.3]")] {
                let s = spec(proto, seed, ONE_INJECTION, traffic);
                run(&s, 60, &format!("{proto} seed {seed} {traffic:?}"));
            }
        }
    }
}

#[test]
fn surge_with_rollbacks_rebuilds_the_full_report() {
    for proto in &PROTOS[2..] {
        for seed in [1, 2] {
            let s = spec(proto, seed, ONE_INJECTION, Some("[0.0,1.0]"));
            let mut live = LiveSmr::new(&s).unwrap();
            let mut mirror = Mirror::new();
            let ctx = format!("{proto} seed {seed}");
            for _ in 0..10 {
                live.step(0.1);
                mirror.sync(&mut live, &ctx);
            }
            live.set_traffic("[0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0,1.0]");
            for _ in 0..40 {
                live.step(0.4);
                mirror.sync(&mut live, &ctx);
            }
            live.set_traffic("[1.0]");
            for _ in 0..40 {
                live.step(0.0);
                mirror.sync(&mut live, &ctx);
            }
        }
    }
}

#[test]
fn dead_terminal_after_pending_commands_rebuilds_the_full_report() {
    for proto in &PROTOS[..2] {
        let injections = r#"[{"round":2,"client":1,"op":7},{"round":5,"client":2,"op":8}]"#;
        let mut live = LiveSmr::new(&spec(proto, 1, injections, None)).unwrap();
        let mut mirror = Mirror::new();
        for _ in 0..3 {
            live.step(0.0);
            mirror.sync(&mut live, proto);
        }
        let dead = parse(&live.step(1.0));
        assert_eq!(dead["dead"], 4, "{proto}: full block kills round 4");
        mirror.sync(&mut live, proto);
        assert_eq!(mirror.merged["commands"][1]["status"], "Dead", "{proto}");
        live.step(0.0);
        mirror.sync(&mut live, proto);
    }
}

#[test]
fn live_inject_sorting_before_a_later_spec_injection_lands_mid_array() {
    for proto in PROTOS {
        let injections = r#"[{"round":2,"client":1,"op":7},{"round":30,"client":2,"op":8}]"#;
        let mut live = LiveSmr::new(&spec(proto, 1, injections, None)).unwrap();
        let mut mirror = Mirror::new();
        mirror.sync(&mut live, proto);
        for _ in 0..5 {
            live.step(0.1);
            mirror.sync(&mut live, proto);
        }
        live.inject(3, 99, None);
        mirror.sync(&mut live, proto);
        let ops: Vec<u64> = mirror.merged["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["op"].as_u64().unwrap())
            .collect();
        assert_eq!(
            ops,
            [7, 99, 8],
            "{proto}: the live command sorts before round 30"
        );
        for _ in 0..40 {
            live.step(0.1);
            mirror.sync(&mut live, proto);
        }
    }
}

#[test]
fn one_delta_spanning_25_steps_rebuilds_the_full_report() {
    for proto in PROTOS {
        let s = spec(proto, 2, ONE_INJECTION, Some("[0.3,0.4,0.3]"));
        let mut live = LiveSmr::new(&s).unwrap();
        let mut mirror = Mirror::new();
        mirror.sync(&mut live, proto);
        for _ in 0..4 {
            for _ in 0..25 {
                live.step(0.1);
            }
            mirror.sync(&mut live, proto);
        }
    }
}

#[test]
fn commands_that_change_after_being_sent_go_out_as_appended_tails() {
    // Recovery re-sends until acked, so a sent command's attempts and landmarks keep moving.
    let s = spec(PROTOS[3], 1, ONE_INJECTION, Some("[0.3,0.4,0.3]"));
    let mut live = LiveSmr::new(&s).unwrap();
    let mut merged = json!({ "metrics": [], "commands": [] });
    let mut changed = 0;
    let mut appended = 0;
    for _ in 0..60 {
        live.step(0.3);
        let delta = parse(&live.report_delta_json());
        changed += delta["changed"].as_array().unwrap().len();
        appended += delta["appended"].as_array().unwrap().len();
        merged = apply(&merged, &delta);
    }
    assert!(changed + appended > 0, "no sent command ever changed");
    assert!(
        appended > 0,
        "no changed command went out in the append form"
    );
    assert_eq!(merged, parse(&live.report_json()));
}

#[test]
fn delta_reads_leave_the_replay_identity_intact() {
    for proto in PROTOS {
        for sticky in [false, true] {
            let s = spec(proto, 11, ONE_INJECTION, Some("[0.7,0.3]"));
            let mut with = LiveSmr::new_with_mode(&s, sticky).unwrap();
            let mut without = LiveSmr::new_with_mode(&s, sticky).unwrap();
            for live in [&mut with, &mut without] {
                live.set_block(2, true);
            }
            for round in 0..30 {
                for live in [&mut with, &mut without] {
                    live.step(0.2);
                    if round == 10 {
                        live.inject(5, 99, None);
                    }
                    if round == 20 {
                        live.set_traffic("[0.5,0.5]");
                    }
                }
                with.report_delta_json();
            }
            assert_eq!(with.export_scenario(), without.export_scenario());
            assert_eq!(
                with.report_json(),
                without.report_json(),
                "{proto} sticky={sticky}"
            );
        }
    }
}
