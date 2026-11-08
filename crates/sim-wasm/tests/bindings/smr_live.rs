//! LiveSmr provenance pins: any live SMR session — including mid-session
//! user injections — exports a spec whose batch replay reproduces the live
//! report byte-identically.

use crate::batch_view::live_report_as_batch;
use sim::smr::ClientModel;
use sim_wasm::{LiveSmr, run_smr_json};

fn spec(proto: &str) -> String {
    format!(
        r#"{{"n":32,"seed":11,"k":6,"ell":3,"sigma":1.5,
            "proto":{proto},
            "injections":[{{"round":2,"client":1,"op":7}}],
            "max_rounds":200,
            "schedule":{{"kind":"fresh_per_round","fraction":0.1}}}}"#
    )
}

fn batch_report(scenario_json: &str) -> serde_json::Value {
    let out: serde_json::Value =
        serde_json::from_str(&run_smr_json(scenario_json)).expect("batch json");
    assert!(out["error"].is_null(), "batch failed: {}", out["error"]);
    out["report"].clone()
}

fn live_report(live: &LiveSmr) -> serde_json::Value {
    live_report_as_batch(live)
}

#[test]
fn fresh_live_session_replays_as_batch() {
    let mut live = LiveSmr::new(&spec(r#"{"kind":"extended"}"#)).unwrap();
    for fraction in [0.0, 0.1, 0.2, 0.1, 0.0, 0.0, 0.1, 0.0] {
        live.step(fraction);
    }
    assert_eq!(batch_report(&live.export_scenario()), live_report(&live));
}

#[test]
fn sticky_with_background_replays_as_batch() {
    let mut live = LiveSmr::new_with_mode(&spec(r#"{"kind":"extended"}"#), true).unwrap();
    for fraction in [0.0, 0.2, 0.2, 0.3, 0.1, 0.0] {
        live.step(fraction);
    }
    assert_eq!(batch_report(&live.export_scenario()), live_report(&live));
}

#[test]
fn mid_session_injections_replay_as_batch() {
    // The headline pin: random-target and explicit-target commands injected
    // live — including a LOW client id arriving late — export into a spec
    // whose batch replay is byte-identical.
    for proto in [
        r#"{"kind":"extended"}"#,
        r#"{"kind":"compact","t_commit_rounds":6}"#,
        r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}"#,
    ] {
        let mut live = LiveSmr::new(&spec(proto)).unwrap();
        for _ in 0..3 {
            live.step(0.1);
        }
        let ok: serde_json::Value =
            serde_json::from_str(&live.inject(5, 99, None)).expect("inject json");
        assert_eq!(ok["round"], 4, "lands next round");
        for _ in 0..2 {
            live.step(0.0);
        }
        let ok2: serde_json::Value =
            serde_json::from_str(&live.inject(0, 77, Some(3))).expect("inject json");
        assert!(ok2["error"].is_null());
        for _ in 0..12 {
            live.step(0.1);
        }
        assert_eq!(
            batch_report(&live.export_scenario()),
            live_report(&live),
            "proto {proto}"
        );
    }
}

#[test]
fn dead_session_absorbs_and_still_replays() {
    for proto in [
        r#"{"kind":"extended"}"#,
        r#"{"kind":"compact","t_commit_rounds":6}"#,
    ] {
        let mut live = LiveSmr::new(&spec(proto)).unwrap();
        let s1: serde_json::Value = serde_json::from_str(&live.step(1.0)).unwrap();
        assert_eq!(s1["dead"], 1, "full block kills round 1");
        assert_eq!(s1["absorbed"], false);
        let s2: serde_json::Value = serde_json::from_str(&live.step(0.0)).unwrap();
        assert_eq!(s2["absorbed"], true, "stepping past death is a no-op");
        assert_eq!(s2["round"], 1);
        assert_eq!(batch_report(&live.export_scenario()), live_report(&live));
    }
}

#[test]
fn inject_user_errors_are_json_not_panics() {
    let mut live = LiveSmr::new(&spec(r#"{"kind":"extended"}"#)).unwrap();
    let dup: serde_json::Value = serde_json::from_str(&live.inject(2, 7, None)).unwrap();
    assert!(dup["error"].as_str().unwrap().contains("op"));
    let range: serde_json::Value = serde_json::from_str(&live.inject(2, 8, Some(32))).unwrap();
    assert!(range["error"].as_str().unwrap().contains("range"));
}

#[test]
fn inject_without_stepping_stays_pending_and_replays() {
    let mut live = LiveSmr::new(&spec(r#"{"kind":"extended"}"#)).unwrap();
    live.inject(2, 8, None);
    let report = live_report(&live);
    assert_eq!(report["commands"][1]["status"], "Pending");
    assert_eq!(batch_report(&live.export_scenario()), report);
}

fn traffic_spec(proto: &str) -> String {
    format!(
        r#"{{"n":32,"seed":11,"k":6,"ell":3,"sigma":1.5,
            "proto":{proto},
            "injections":[{{"round":2,"client":1,"op":7}}],
            "max_rounds":200,
            "schedule":{{"kind":"fresh_per_round","fraction":0.1}},
            "traffic":{{"kind":"pmf","arrivals_pmf":[0.0,1.0]}}}}"#
    )
}

#[test]
fn traffic_session_with_midrun_edit_and_inject_replays_as_batch() {
    for (proto, sticky) in [
        (r#"{"kind":"extended"}"#, false),
        (r#"{"kind":"extended"}"#, true),
        (r#"{"kind":"compact","t_commit_rounds":8}"#, false),
        (
            r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}"#,
            false,
        ),
        (
            r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}"#,
            true,
        ),
    ] {
        let mut live = LiveSmr::new_with_mode(&traffic_spec(proto), sticky).unwrap();
        for _ in 0..5 {
            live.step(0.1);
        }
        let ok: serde_json::Value = serde_json::from_str(&live.set_traffic("[1.0]")).unwrap();
        assert!(
            ok["error"].is_null(),
            "live pmf edit failed: {}",
            ok["error"]
        );
        let inj: serde_json::Value = serde_json::from_str(&live.inject(2, 99, None)).unwrap();
        assert!(
            inj["error"].is_null(),
            "mid-run inject failed: {}",
            inj["error"]
        );
        for _ in 0..5 {
            live.step(0.1);
        }

        let export: serde_json::Value = serde_json::from_str(&live.export_scenario()).unwrap();
        assert_eq!(
            export["traffic"]["kind"], "phases",
            "mutated session exports the exact phase list"
        );
        assert_eq!(export["traffic"]["phases"].as_array().unwrap().len(), 2);
        let clients: Vec<u64> = export["injections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["client"].as_u64().unwrap())
            .collect();
        assert_eq!(
            clients,
            vec![1, 2],
            "exported injections are manual-only — arrivals regenerate from phases"
        );
        assert_eq!(
            batch_report(&live.export_scenario()),
            live_report(&live),
            "proto {proto} sticky {sticky}"
        );
    }
}

#[test]
fn a_non_pool_session_exports_no_client_model() {
    let mut live =
        LiveSmr::new(&traffic_spec(r#"{"kind":"compact","t_commit_rounds":8}"#)).unwrap();
    for _ in 0..3 {
        live.step(0.0);
    }
    assert!(
        !live.export_scenario().contains("client_model"),
        "an absent client model must stay absent through a live session's export"
    );
}

#[test]
fn unmutated_traffic_session_exports_pmf_sugar() {
    let mut live = LiveSmr::new(&traffic_spec(r#"{"kind":"extended"}"#)).unwrap();
    for _ in 0..3 {
        live.step(0.0);
    }
    let export: serde_json::Value = serde_json::from_str(&live.export_scenario()).unwrap();
    assert_eq!(export["traffic"]["kind"], "pmf");
    assert_eq!(batch_report(&live.export_scenario()), live_report(&live));
}

fn compact_spec() -> String {
    r#"{"n":16,"seed":77,"k":6,"ell":3,"sigma":1.5,
        "proto":{"kind":"compact","t_commit_rounds":20},
        "injections":[],
        "max_rounds":400,
        "schedule":{"kind":"fresh_per_round","fraction":0.0}}"#
        .to_string()
}

#[test]
fn certs_json_exposes_the_forest_and_matches_an_independent_recomputation() {
    let mut live = LiveSmr::new(&compact_spec()).unwrap();
    // Parallel engine on the identical stream — the independent ground truth.
    let scenario = {
        let spec: sim::spec::SmrScenarioSpec = serde_json::from_str(&compact_spec()).unwrap();
        sim::smr::SmrScenario::try_from(spec).unwrap()
    };
    let mut twin = sim::smr::SmrState::new(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &[],
        &[],
        ClientModel::Unique,
        None,
    );

    live.inject(5, 100, None);
    twin.inject(5, 100, None).unwrap();
    let mut merge_rounds = 0usize;
    for i in 0..80u64 {
        if i == 20 {
            live.inject(5, 101, None);
            twin.inject(5, 101, None).unwrap();
        }
        live.step(0.0);
        twin.draw_arrivals();
        twin.step_sampled(0);

        let snap: serde_json::Value = serde_json::from_str(&live.certs_json()).unwrap();
        assert!(snap["error"].is_null(), "no error in compact mode: {snap}");
        // (b) fault-free run: roots always consistent.
        assert_eq!(snap["roots_consistent"], true);

        for server in snap["servers"].as_array().unwrap() {
            let m = server["m"].as_u64().unwrap();
            // (a) peak heights are exactly the set bits of m, tallest first.
            let heights: Vec<u32> = server["peaks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["height"].as_u64().unwrap() as u32)
                .collect();
            let expect: Vec<u32> = (0..64).rev().filter(|b| (m >> b) & 1 == 1).collect();
            assert_eq!(heights, expect, "m = {m}");
        }
        // (c) merge events follow binary carries on the twin's executed growth.
        merge_rounds += usize::from(!snap["merges_last_step"].as_array().unwrap().is_empty());

        // (d) roots match a from-scratch recomputation over the same entries.
        let executed = twin.executed_seqs();
        let servers = snap["servers"].as_array().unwrap();
        for server in servers {
            let idx = server["server"].as_u64().unwrap() as usize;
            let mut forest = protocol::certificates::MmrForest::new();
            for entry in &executed[idx] {
                forest.append(protocol::certificates::leaf_hash(entry));
            }
            let roots_hex: Vec<String> = forest
                .roots()
                .iter()
                .map(|r| r.iter().map(|b| format!("{b:02x}")).collect())
                .collect();
            let got: Vec<String> = server["peaks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["root_hex"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(got, roots_hex, "server {idx}");
        }
    }
    assert!(merge_rounds > 0, "some step produced carry merges");

    // Clients section reflects the injected sn sequence.
    let snap: serde_json::Value = serde_json::from_str(&live.certs_json()).unwrap();
    let clients = snap["clients"].as_array().unwrap();
    let c5 = clients.iter().find(|c| c["client"] == 5).expect("client 5");
    let sns: Vec<u64> = c5["certs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["sn"].as_u64().unwrap())
        .collect();
    assert_eq!(sns, vec![1, 2]);
    assert!(c5["certs"][0]["committed"].as_bool().unwrap());

    // Read-only accessor: the session still replays byte-identically.
    assert_eq!(batch_report(&live.export_scenario()), live_report(&live));
}

#[test]
fn certs_json_errors_on_the_extended_rule() {
    let mut live = LiveSmr::new(&spec(r#"{"kind":"extended"}"#)).unwrap();
    live.step(0.0);
    let snap: serde_json::Value = serde_json::from_str(&live.certs_json()).unwrap();
    assert!(snap["error"].as_str().unwrap().contains("compact"));
}

fn certs_snap(live: &LiveSmr) -> serde_json::Value {
    serde_json::from_str(&live.certs_json()).expect("certs json")
}

fn client_obj(snap: &serde_json::Value, client: u64) -> serde_json::Value {
    snap["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["client"] == client)
        .unwrap_or_else(|| panic!("client {client} in snapshot"))
        .clone()
}

fn step_until_committed(live: &mut LiveSmr, client: u64, sn: u64) {
    for _ in 0..200 {
        let c = client_obj(&certs_snap(live), client);
        let done = c["certs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["sn"] == sn && r["committed"] == true);
        if done {
            return;
        }
        live.step(0.0);
    }
    panic!("client {client} sn {sn} never committed");
}

#[test]
fn capture_stale_pins_a_newest_cert_that_dies_after_two_more_commits() {
    let mut live = LiveSmr::new(&compact_spec()).unwrap();
    live.inject(5, 100, None);
    step_until_committed(&mut live, 5, 1);

    let cap: serde_json::Value = serde_json::from_str(&live.capture_stale(5)).unwrap();
    assert!(cap["error"].is_null(), "capture failed: {}", cap["error"]);
    assert_eq!(cap["sn"], 1);

    // Freshly captured: x1 still sits in every useful server's last-two window.
    let c5 = client_obj(&certs_snap(&live), 5);
    let stale = &c5["stale"];
    assert_eq!(stale["sn"], 1);
    let covered = stale["covered"].as_u64().unwrap();
    assert_eq!(covered, 16, "fault-free run: every server covers");
    assert_eq!(stale["accepted"], covered);

    // Two further commits evict x1 from the last-two window (§5 staleness).
    live.inject(5, 101, None);
    step_until_committed(&mut live, 5, 2);
    live.inject(5, 102, None);
    step_until_committed(&mut live, 5, 3);

    let c5 = client_obj(&certs_snap(&live), 5);
    let stale = &c5["stale"];
    assert_eq!(stale["sn"], 1, "the frozen row keeps its sn");
    assert!(stale["covered"].as_u64().unwrap() > 0);
    assert_eq!(stale["accepted"], 0, "evicted cert verifies nowhere");

    // Read-only cert APIs left the stream untouched.
    assert_eq!(batch_report(&live.export_scenario()), live_report(&live));
}

#[test]
fn uncaptured_clients_carry_a_null_stale_field() {
    let mut live = LiveSmr::new(&compact_spec()).unwrap();
    live.inject(5, 100, None);
    step_until_committed(&mut live, 5, 1);
    let c5 = client_obj(&certs_snap(&live), 5);
    assert!(c5["stale"].is_null());
}

#[test]
fn verify_json_tallies_committed_rows_and_skips_certless_rows() {
    let mut live = LiveSmr::new(&compact_spec()).unwrap();
    live.inject(5, 100, None);
    step_until_committed(&mut live, 5, 1);
    live.inject(5, 101, None);
    step_until_committed(&mut live, 5, 2);
    live.inject(5, 102, None);
    step_until_committed(&mut live, 5, 3);
    // A fourth command, never stepped: issued but pending — no cert exists.
    live.inject(5, 103, None);

    let v: serde_json::Value = serde_json::from_str(&live.verify_json()).unwrap();
    assert!(v["error"].is_null(), "verify failed: {}", v["error"]);
    assert!(v["round"].as_u64().unwrap() > 0);
    let c5 = v["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["client"] == 5)
        .expect("client 5");
    let rows = c5["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 4);

    // sn 1..2: chained certs, fault-free — every server covers and accepts.
    for row in &rows[..2] {
        let t = &row["tally"];
        assert_eq!(t["covered"], 16, "row {row}");
        assert_eq!(t["accepted"], 16, "row {row}");
    }
    // sn 3: the newest acked command — bare-newest cert, still fully valid.
    assert_eq!(rows[2]["tally"]["covered"], 16);
    assert_eq!(rows[2]["tally"]["accepted"], 16);
    // sn 4: pending, no certificate constructible yet.
    assert!(rows[3]["tally"].is_null());

    // Pure read: the session still replays byte-identically.
    assert_eq!(batch_report(&live.export_scenario()), live_report(&live));
}

#[test]
fn capture_and_verify_user_errors_are_json_not_panics() {
    // Extended rule: no executed state, both APIs refuse.
    let mut ext = LiveSmr::new(&spec(r#"{"kind":"extended"}"#)).unwrap();
    ext.step(0.0);
    let cap: serde_json::Value = serde_json::from_str(&ext.capture_stale(1)).unwrap();
    assert!(cap["error"].as_str().unwrap().contains("compact"));
    let ver: serde_json::Value = serde_json::from_str(&ext.verify_json()).unwrap();
    assert!(ver["error"].as_str().unwrap().contains("compact"));

    // Compact: unknown client, and a client with nothing committed yet.
    let mut live = LiveSmr::new(&compact_spec()).unwrap();
    let unknown: serde_json::Value = serde_json::from_str(&live.capture_stale(9)).unwrap();
    assert!(unknown["error"].as_str().unwrap().contains("client"));
    live.inject(5, 100, None);
    let early: serde_json::Value = serde_json::from_str(&live.capture_stale(5)).unwrap();
    assert!(early["error"].as_str().unwrap().contains("commit"));
}

#[test]
fn certs_json_committed_maps_positions_to_commands() {
    let mut live = LiveSmr::new(&compact_spec()).unwrap();
    let scenario = {
        let spec: sim::spec::SmrScenarioSpec = serde_json::from_str(&compact_spec()).unwrap();
        sim::smr::SmrScenario::try_from(spec).unwrap()
    };
    let mut twin = sim::smr::SmrState::new(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &[],
        &[],
        ClientModel::Unique,
        None,
    );

    live.inject(5, 100, None);
    twin.inject(5, 100, None).unwrap();
    for i in 0..80u64 {
        if i == 20 {
            live.inject(5, 101, None);
            twin.inject(5, 101, None).unwrap();
        }
        live.step(0.0);
        twin.draw_arrivals();
        twin.step_sampled(0);

        let snap = certs_snap(&live);
        let committed = snap["committed"].as_array().unwrap();
        let server = snap["committed_server"].as_u64().unwrap() as usize;

        // Ground truth: the executed prefix of the first max-m server —
        // the same server whose forest the lab draws.
        let executed = twin.executed_seqs();
        let best = executed
            .iter()
            .enumerate()
            .max_by_key(|(i, e)| (e.len(), std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(server, best);
        assert_eq!(committed.len(), executed[best].len());

        for (entry, got) in executed[best].iter().zip(committed) {
            match entry {
                protocol::compact::Entry::Cmd(cc) => {
                    assert_eq!(got["kind"], "cmd");
                    assert_eq!(got["client"].as_u64().unwrap() as u32, cc.client);
                    assert_eq!(got["sn"].as_u64().unwrap(), cc.sn);
                }
                protocol::compact::Entry::Null { client, sn } => {
                    assert_eq!(got["kind"], "null");
                    assert_eq!(got["client"].as_u64().unwrap() as u32, *client);
                    assert_eq!(got["sn"].as_u64().unwrap(), *sn);
                }
                protocol::compact::Entry::Nop(_) => {
                    assert_eq!(got["kind"], "nop");
                    assert!(got["client"].is_null());
                }
            }
        }
    }

    // Both injected commands ended up at exactly one position each.
    let snap = certs_snap(&live);
    let cmds: Vec<(u64, u64)> = snap["committed"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "cmd" && e["client"] == 5)
        .map(|e| (e["client"].as_u64().unwrap(), e["sn"].as_u64().unwrap()))
        .collect();
    assert_eq!(cmds, vec![(5, 1), (5, 2)]);

    // Pure read: the session still replays byte-identically.
    assert_eq!(batch_report(&live.export_scenario()), live_report(&live));
}

// --- recovery drilldown: node_detail is recovery-gated and read-only ---

fn recovery_spec() -> String {
    spec(r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}"#)
}

#[test]
fn node_detail_reports_recovery_state_and_never_perturbs_the_stream() {
    let mut live = LiveSmr::new(&recovery_spec()).unwrap();
    for _ in 0..12 {
        live.step(0.1);
    }
    let d: serde_json::Value = serde_json::from_str(&live.node_detail(0)).expect("detail json");
    assert!(d["error"].is_null(), "recovery has detail: {}", d["error"]);
    for key in [
        "node",
        "r",
        "log_len",
        "executed_len",
        "checkpoint_window",
        "checkpoint_p_len",
        "s_hash",
        "checkpoint_s_hash",
    ] {
        assert!(d.get(key).is_some(), "detail is missing {key}");
    }
    assert_eq!(d["node"], 0);
    assert_eq!(d["s_hash"].as_str().expect("hex string").len(), 16);

    let bad: serde_json::Value = serde_json::from_str(&live.node_detail(32)).expect("json");
    assert!(
        bad["error"].is_string(),
        "an unknown node id is an error, not a panic"
    );

    // Read-only: hammering the drilldown mid-session must leave the exported
    // session replaying byte-identically.
    for i in 0..32 {
        live.node_detail(i);
    }
    for _ in 0..8 {
        live.step(0.2);
        live.node_detail(1);
    }
    assert_eq!(batch_report(&live.export_scenario()), live_report(&live));
}

#[test]
fn node_detail_errors_outside_the_recovery_rule() {
    for proto in [
        r#"{"kind":"extended"}"#,
        r#"{"kind":"compact","t_commit_rounds":6}"#,
    ] {
        let mut live = LiveSmr::new(&spec(proto)).unwrap();
        live.step(0.0);
        let out: serde_json::Value = serde_json::from_str(&live.node_detail(0)).expect("json");
        assert!(
            out["error"].is_string(),
            "{proto}: no recovery state to show"
        );
    }
}

// --- targeted blocking: lands next round, exports as a replayable overlay ---

fn blocked_ids(status_json: &str) -> Vec<u64> {
    let status: serde_json::Value = serde_json::from_str(status_json).expect("status json");
    status["blocked"]
        .as_array()
        .expect("blocked array")
        .iter()
        .map(|v| v.as_u64().expect("node id"))
        .collect()
}

fn ok_json(out: &str) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_str(out).expect("json");
    assert!(
        value["error"].is_null(),
        "unexpected error: {}",
        value["error"]
    );
    value
}

#[test]
fn set_block_masks_its_node_from_the_next_round_until_it_is_released() {
    let mut live = LiveSmr::new(&recovery_spec()).unwrap();
    assert!(blocked_ids(&live.step(0.0)).is_empty(), "round 1 is clean");

    assert_eq!(
        ok_json(&live.set_block(3, true))["round"],
        2,
        "lands next round"
    );
    assert_eq!(blocked_ids(&live.step(0.0)), vec![3]);
    assert_eq!(blocked_ids(&live.step(0.0)), vec![3], "and stays");

    assert_eq!(ok_json(&live.set_block(3, false))["round"], 4);
    assert!(
        blocked_ids(&live.step(0.0)).is_empty(),
        "released in round 4"
    );

    // A block cancelled before it ever ran leaves no trace to replay.
    live.set_block(9, true);
    live.set_block(9, false);

    let spec: serde_json::Value = serde_json::from_str(&live.export_scenario()).expect("spec");
    assert_eq!(
        spec["manual_blocks"],
        serde_json::json!([{"node": 3, "from_round": 2, "to_round": 4}])
    );

    let bad: serde_json::Value = serde_json::from_str(&live.set_block(32, true)).expect("json");
    assert!(bad["error"].is_string(), "an unknown node id is an error");
}

#[test]
fn a_session_that_never_blocks_exports_no_overlay() {
    let mut live = LiveSmr::new(&recovery_spec()).unwrap();
    live.step(0.1);
    assert!(
        !live.export_scenario().contains("manual_blocks"),
        "an empty overlay stays off the wire"
    );
}

#[test]
fn targeted_blocking_replays_as_batch_in_both_arms() {
    // The headline provenance pin: whatever the operator blocked mid-session
    // is reconstructed from the exported spec alone.
    for sticky in [false, true] {
        let mut live = LiveSmr::new_with_mode(&recovery_spec(), sticky).unwrap();
        for _ in 0..4 {
            live.step(0.1);
        }
        ok_json(&live.set_block(3, true));
        for _ in 0..6 {
            live.step(0.1);
        }
        ok_json(&live.set_block(7, true));
        for _ in 0..3 {
            live.step(0.25);
        }
        ok_json(&live.set_block(3, false));
        for _ in 0..7 {
            live.step(0.1);
        }
        assert_eq!(
            batch_report(&live.export_scenario()),
            live_report(&live),
            "sticky={sticky}"
        );
    }
}

#[test]
fn verify_json_covers_no_server_blocked_in_the_last_round() {
    let recovery = compact_spec().replace(
        r#"{"kind":"compact","t_commit_rounds":20}"#,
        r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":true}"#,
    );
    for spec in [compact_spec(), recovery] {
        covered_excludes_the_blocked(&spec);
    }
}

fn covered_excludes_the_blocked(spec: &str) {
    let mut live = LiveSmr::new(spec).unwrap();
    live.inject(5, 100, None);
    step_until_committed(&mut live, 5, 1);
    live.set_block(3, true);
    let status: serde_json::Value = serde_json::from_str(&live.step(0.25)).unwrap();
    let blocked = status["blocked"].as_array().unwrap().len() as u64;
    assert!(blocked >= 4, "node 3 plus the sampled quarter");

    let v: serde_json::Value = serde_json::from_str(&live.verify_json()).unwrap();
    let row = &v["clients"][0]["rows"][0];
    let covered = row["tally"]["covered"].as_u64().unwrap();
    assert!(covered <= 16 - blocked, "row {row}, blocked {blocked}");
}

#[test]
fn certificate_reads_refuse_after_a_prefix_violation() {
    let violating = r#"{"n":32,"seed":3,"k":6,"ell":3,"sigma":1.0,
        "proto":{"kind":"compact","t_commit_rounds":1},
        "injections":[{"round":1,"client":1,"op":7}],
        "traffic":{"kind":"pmf","arrivals_pmf":[0.0,0.0,1.0]},
        "max_rounds":40,
        "schedule":{"kind":"fresh_per_round","fraction":0.0}}"#;
    let mut live = LiveSmr::new(violating).unwrap();
    for _ in 0..3 {
        live.step(0.0);
    }
    let report: serde_json::Value = serde_json::from_str(&live.report_json()).unwrap();
    assert_eq!(report["safety_ok"], false, "the probe must violate safety");
    for out in [live.verify_json(), live.capture_stale(1)] {
        let out: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            out["error"].as_str().is_some_and(|e| e.contains("prefix")),
            "{out}"
        );
    }
}

// The lab mounts §5 after the spec is parsed, so a spec the batch tier accepts can still be
// refused by the layer it gets: that must reach the page as an error, never as a trap.
const RECOVERY_SKIP: &str = r#"{"n":16,"seed":3,"k":6,"ell":3,"sigma":1.0,
    "proto":{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false},
    "repeated_commit":"skip","injections":[],"max_rounds":20,
    "schedule":{"kind":"fresh_per_round","fraction":0.0}}"#;

#[test]
fn recovery_with_repeated_commit_skip_is_an_error_in_the_batch_entry() {
    let out: serde_json::Value = serde_json::from_str(&run_smr_json(RECOVERY_SKIP)).expect("json");
    let error = out["error"].as_str().expect("an error object");
    assert!(error.contains("certificate"), "{error}");
}

#[test]
fn recovery_with_repeated_commit_skip_is_an_error_in_the_live_session() {
    let Err(error) = LiveSmr::new(RECOVERY_SKIP) else {
        panic!("the live session must refuse the spec");
    };
    assert!(error.contains("certificate"), "{error}");
}
