//! Live sessions under the bounded client pool: a reused client's commands stay apart in every
//! live output (spreading entries, certificates, the committed chain), and the session still
//! replays byte-identically from its export.

use crate::batch_view::live_report_as_batch;
use serde_json::Value;
use sim::smr::AUTO_CLIENT_BASE;
use sim_wasm::{LiveSmr, run_smr_json};
use std::collections::BTreeMap;

const COMPACT: &str = r#"{"kind":"compact","t_commit_rounds":8}"#;
const RECOVERY: &str = r#"{"kind":"recovery","t_window_rounds":16,"resend_until_acked":false}"#;
const RECOVERY_ACK: &str = r#"{"kind":"recovery","t_window_rounds":16,"resend_until_acked":true}"#;
const PROTOS: [&str; 3] = [COMPACT, RECOVERY, RECOVERY_ACK];

fn pool_spec(proto: &str, clients: u32) -> String {
    format!(
        r#"{{"n":32,"seed":5,"k":6,"ell":3,"sigma":3.0,"proto":{proto},
            "injections":[{{"round":2,"client":1,"op":7}}],
            "max_rounds":400,
            "schedule":{{"kind":"fresh_per_round","fraction":0.05}},
            "traffic":{{"kind":"pmf","arrivals_pmf":[0.7,0.3]}},
            "client_model":{{"kind":"pool","clients":{clients}}}}}"#
    )
}

fn parse(json: &str) -> Value {
    serde_json::from_str(json).expect("json")
}

fn batch_report(scenario_json: &str) -> Value {
    let out = parse(&run_smr_json(scenario_json));
    assert!(out["error"].is_null(), "batch failed: {}", out["error"]);
    out["report"].clone()
}

/// Report commands per client, in report order.
fn by_client(report: &Value) -> BTreeMap<u64, Vec<Value>> {
    let mut map: BTreeMap<u64, Vec<Value>> = BTreeMap::new();
    for c in report["commands"].as_array().unwrap() {
        map.entry(c["client"].as_u64().unwrap())
            .or_default()
            .push(c.clone());
    }
    map
}

fn assert_reuses_clients(report: &Value) {
    let reused = by_client(report)
        .iter()
        .filter(|(client, cmds)| **client >= u64::from(AUTO_CLIENT_BASE) && cmds.len() >= 3)
        .count();
    assert!(reused > 0, "the shape must reuse pool clients");
}

#[test]
fn a_live_pool_session_replays_as_batch_and_exports_its_client_model() {
    for proto in PROTOS {
        let mut live = LiveSmr::new(&pool_spec(proto, 48)).expect("pool runs live");
        for round in 0..200 {
            if round == 60 {
                assert!(parse(&live.inject(1, 8, None))["error"].is_null());
            }
            if round == 90 {
                assert!(parse(&live.set_traffic("[0.6,0.4]"))["error"].is_null());
            }
            live.step(if (100..120).contains(&round) {
                0.2
            } else {
                0.05
            });
        }
        let export = parse(&live.export_scenario());
        assert_eq!(
            export["client_model"],
            serde_json::json!({"kind": "pool", "clients": 48}),
            "{proto}"
        );
        let report = live_report_as_batch(&live);
        assert_reuses_clients(&report);
        assert_eq!(
            batch_report(&live.export_scenario()),
            report,
            "proto {proto}"
        );
    }
}

#[test]
fn every_spreading_entry_names_one_command_of_the_report() {
    for proto in PROTOS {
        let mut live = LiveSmr::new(&pool_spec(proto, 48)).unwrap();
        let mut statuses = Vec::new();
        for _ in 0..200 {
            statuses.push(parse(&live.step(0.05)));
        }
        let report = parse(&live.report_json());
        let client_of: BTreeMap<u64, u64> = report["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["op"].as_u64().unwrap(), c["client"].as_u64().unwrap()))
            .collect();
        for status in statuses {
            let mut ops = std::collections::BTreeSet::new();
            for s in status["spreading"].as_array().unwrap() {
                let op = s["op"].as_u64().expect("each entry carries its op");
                assert!(ops.insert(op), "{proto}: op {op} listed twice");
                assert_eq!(Some(&s["client"].as_u64().unwrap()), client_of.get(&op));
            }
        }
    }
}

/// The lab derives a command's sn from its rank among its client's report commands; the
/// committed chain carries the engine's own (client, sn, op), which must agree.
#[test]
fn a_pool_clients_report_rank_is_its_engine_sn() {
    for proto in [RECOVERY, RECOVERY_ACK] {
        let mut live = LiveSmr::new(&pool_spec(proto, 48)).unwrap();
        for _ in 0..200 {
            live.step(0.05);
        }
        let report = parse(&live.report_json());
        let clients = by_client(&report);
        let chain = parse(&live.chain_json(0));
        let mut checked = 0;
        for e in chain["entries"].as_array().unwrap() {
            if e["kind"] != "cmd" {
                continue;
            }
            let rank = e["sn"].as_u64().unwrap() as usize - 1;
            let cmd = &clients[&e["client"].as_u64().unwrap()][rank];
            assert_eq!(cmd["op"], e["op"], "{proto}: {e}");
            checked += usize::from(rank > 0);
        }
        assert!(
            checked > 0,
            "{proto}: the chain must commit a reused client's later sn"
        );
    }
}

/// Under §5 a pool client is an ordinary repeated client: one certificate row per command it
/// issued, acks in sn order, and a fault-free verify accepted wherever it is covered.
#[test]
fn pool_clients_carry_certificates_over_compact_and_recovery() {
    for proto in [COMPACT, RECOVERY_ACK] {
        let spec = pool_spec(proto, 48).replace(r#""fraction":0.05"#, r#""fraction":0.0"#);
        let mut live = LiveSmr::new(&spec).unwrap();
        for _ in 0..200 {
            live.step(0.0);
        }
        let report = parse(&live.report_json());
        let clients = by_client(&report);
        let certs = parse(&live.certs_json());
        assert!(certs["error"].is_null(), "{proto}: {certs}");
        let pool_rows: Vec<&Value> = certs["clients"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["client"].as_u64().unwrap() >= u64::from(AUTO_CLIENT_BASE))
            .collect();
        assert!(
            !pool_rows.is_empty(),
            "{proto}: pool clients are registered"
        );
        let mut repeated = 0;
        for row in &pool_rows {
            let client = row["client"].as_u64().unwrap();
            let cmds = &clients[&client];
            let sns: Vec<u64> = row["certs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["sn"].as_u64().unwrap())
                .collect();
            assert_eq!(sns, (1..=cmds.len() as u64).collect::<Vec<_>>(), "{proto}");
            let acked = cmds
                .iter()
                .take_while(|c| !c["committed_ack_round"].is_null())
                .count() as u64;
            assert_eq!(
                row["next_sn"].as_u64().unwrap(),
                acked + 1,
                "{proto} c{client}"
            );
            repeated += usize::from(acked >= 2);
        }
        assert!(repeated > 0, "{proto}: some pool client must commit twice");

        let verify = parse(&live.verify_json());
        let mut accepted_later_sn = 0;
        for c in verify["clients"].as_array().unwrap() {
            for r in c["rows"].as_array().unwrap() {
                let t = &r["tally"];
                if t.is_null() {
                    continue;
                }
                assert_eq!(t["accepted"], t["covered"], "{proto}: {c}");
                accepted_later_sn += usize::from(r["sn"].as_u64().unwrap() >= 2);
            }
        }
        assert!(
            accepted_later_sn > 0,
            "{proto}: a later-sn certificate verifies"
        );
    }
}

/// §5's staleness demo on a pool client: a captured bare-newest certificate verifies until two
/// further commits of that client move every server's window past it.
#[test]
fn a_captured_pool_certificate_goes_stale_after_two_more_commits() {
    let spec = pool_spec(RECOVERY_ACK, 12)
        .replace(r#""fraction":0.05"#, r#""fraction":0.0"#)
        .replace("[0.7,0.3]", "[0.9,0.1]");
    let mut live = LiveSmr::new(&spec).unwrap();
    let first_acked_pool_client = |live: &LiveSmr| {
        parse(&live.certs_json())["clients"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["client"].as_u64().unwrap() >= u64::from(AUTO_CLIENT_BASE)
                    && c["next_sn"].as_u64().unwrap() > 1
            })
            .map(|c| c["client"].as_u64().unwrap() as u32)
    };
    let client = loop {
        if let Some(client) = first_acked_pool_client(&live) {
            break client;
        }
        live.step(0.0);
    };
    let acked = |live: &LiveSmr| {
        parse(&live.certs_json())["clients"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["client"] == client)
            .map_or(0, |c| c["next_sn"].as_u64().unwrap() - 1)
    };
    let captured = parse(&live.capture_stale(client))["sn"].as_u64().unwrap();
    let stale = |live: &LiveSmr| {
        parse(&live.certs_json())["clients"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["client"] == client)
            .unwrap()["stale"]
            .clone()
    };
    let fresh = stale(&live);
    assert_eq!(
        fresh["accepted"], fresh["covered"],
        "fresh capture verifies"
    );
    for _ in 0..1500 {
        if acked(&live) >= captured + 2 {
            break;
        }
        live.step(0.0);
    }
    assert!(
        acked(&live) >= captured + 2,
        "the client commits twice more"
    );
    for _ in 0..40 {
        live.step(0.0);
    }
    assert_eq!(
        stale(&live)["accepted"],
        0,
        "evicted from every server's last-two window"
    );
}

#[test]
fn an_exported_pool_session_restarts_live_and_steps_identically() {
    let mut live = LiveSmr::new(&pool_spec(RECOVERY_ACK, 48)).unwrap();
    let fractions: Vec<f64> = (0..150)
        .map(|r| if r % 7 == 0 { 0.1 } else { 0.0 })
        .collect();
    for &f in &fractions {
        live.step(f);
    }
    let mut again = LiveSmr::new(&live.export_scenario()).expect("the export reimports live");
    for &f in &fractions {
        again.step(f);
    }
    assert_eq!(again.report_json(), live.report_json());
    assert_eq!(again.certs_json(), live.certs_json());
}

#[test]
fn an_extended_pool_is_refused_with_the_engines_reason() {
    let Err(err) = LiveSmr::new(&pool_spec(r#"{"kind":"extended"}"#, 8)) else {
        panic!("extended has no latch to free a pool client on");
    };
    assert!(err.contains("compact/recovery only"), "got: {err}");
    let batch = parse(&run_smr_json(&pool_spec(r#"{"kind":"extended"}"#, 8)));
    assert!(
        batch["error"]
            .as_str()
            .unwrap()
            .contains("compact/recovery only"),
        "the batch entry refuses it too: {batch}"
    );
}

/// Checked every round through a surge that rolls checkpoints back: each pool client lists one
/// certificate row per report command, its acks form a prefix in sn order, and `next_sn` follows.
/// A surge stops commits, so the pool must hold every arrival of the surge.
#[test]
fn the_pool_certificate_harness_matches_the_report_every_round() {
    let mut live = LiveSmr::new(&pool_spec(RECOVERY_ACK, 400)).unwrap();
    for round in 1..=220 {
        live.step(if (90..130).contains(&round) {
            0.5
        } else {
            0.05
        });
        let certs = parse(&live.certs_json());
        if !certs["error"].is_null() {
            continue;
        }
        let clients = by_client(&parse(&live.report_json()));
        for row in certs["clients"].as_array().unwrap() {
            let client = row["client"].as_u64().unwrap();
            if client < u64::from(AUTO_CLIENT_BASE) {
                continue;
            }
            let cmds = &clients[&client];
            let rows = row["certs"].as_array().unwrap();
            assert_eq!(rows.len(), cmds.len(), "round {round} c{client}");
            let acked = cmds
                .iter()
                .take_while(|c| !c["committed_ack_round"].is_null())
                .count();
            assert!(
                cmds[acked..]
                    .iter()
                    .all(|c| c["committed_ack_round"].is_null()),
                "round {round} c{client}: acks form a prefix"
            );
            assert_eq!(row["next_sn"].as_u64().unwrap(), acked as u64 + 1);
            for (k, r) in rows.iter().enumerate() {
                assert_eq!(r["committed"], k < acked, "round {round} c{client}");
            }
        }
    }
    let report = parse(&live.report_json());
    let rollbacks: u64 = report["recovery"]["rounds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["rollbacks"].as_u64().unwrap())
        .sum();
    assert!(rollbacks > 0, "the surge must roll checkpoints back");
}

/// An undersized pool stops the live session with the engine's own message, as a batch run does.
#[test]
#[should_panic(expected = "client pool exhausted")]
fn an_exhausted_live_pool_stops_with_the_engines_message() {
    let mut live = LiveSmr::new(&pool_spec(RECOVERY_ACK, 2)).unwrap();
    for _ in 0..200 {
        live.step(0.05);
    }
}

#[test]
fn an_explicit_unique_model_runs_as_unique() {
    let unique = pool_spec(RECOVERY_ACK, 8)
        .replace(r#"{"kind":"pool","clients":8}"#, r#"{"kind":"unique"}"#);
    let mut plain_spec = parse(&unique);
    assert_eq!(
        plain_spec.as_object_mut().unwrap().remove("client_model"),
        Some(serde_json::json!({"kind": "unique"}))
    );
    let plain_spec = plain_spec.to_string();
    let mut live = LiveSmr::new(&unique).unwrap();
    let mut plain = LiveSmr::new(&plain_spec).unwrap();
    for _ in 0..80 {
        assert_eq!(live.step(0.05), plain.step(0.05));
    }
    assert_eq!(live.report_json(), plain.report_json());
    assert_eq!(live.certs_json(), plain.certs_json());
}
