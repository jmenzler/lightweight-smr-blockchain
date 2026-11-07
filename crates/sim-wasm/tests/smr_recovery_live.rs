//! The four recovery oracles the lab's correctness rests on. The lab has no
//! JS test rig by design, so anything checkable lives here: replay identity,
//! a frozen rec-row arc, non-affection of the other rules, and a twin of the
//! r*̂ scan the lab draws its law card from.

use sim::smr::{SmrScenario, run_smr};
use sim::spec::SmrScenarioSpec;
use sim_wasm::{LiveSmr, run_smr_json};

fn recovery_spec(t_window: u64) -> String {
    format!(
        r#"{{"n":32,"seed":11,"k":6,"ell":3,"sigma":1.5,
            "proto":{{"kind":"recovery","t_window_rounds":{t_window},"resend_until_acked":false}},
            "injections":[{{"round":2,"client":1,"op":7}}],
            "max_rounds":200,
            "traffic":{{"kind":"pmf","arrivals_pmf":[0.7,0.3]}},
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
    serde_json::from_str(&live.report_json()).expect("live report json")
}

fn batch(spec_json: &str) -> sim::smr::SmrReport {
    let spec: SmrScenarioSpec = serde_json::from_str(spec_json).expect("spec parses");
    run_smr(&SmrScenario::try_from(spec).expect("valid scenario"))
}

fn failing_spec() -> String {
    r#"{"n":598,"seed":1,"k":6,"ell":3,"sigma":1.0,
        "proto":{"kind":"recovery","t_window_rounds":20,"resend_until_acked":true},
        "injections":[{"round":2,"client":1,"op":1},{"round":2,"client":2,"op":2}],
        "max_rounds":200,
        "schedule":{"kind":"fresh_per_round","fraction":0.1},
        "traffic":{"kind":"pmf","arrivals_pmf":[0.0,0.0,1.0]}}"#
        .to_string()
}

#[test]
fn typed_failure_is_a_replayable_absorbing_live_result() {
    let spec = failing_spec();
    let batch = batch_report(&spec);
    let terminal = &batch["terminal"]["Failed"];
    let attempted = terminal["attempted_round"].as_u64().expect("failed round");
    assert!((20..=200).contains(&attempted));
    assert_eq!(attempted % 20, 0);
    assert_eq!(terminal["completed_round"], attempted - 1);
    assert_eq!(terminal["observed_round"], attempted);
    assert_eq!(terminal["phase"], "BoundaryPreflight");
    assert_eq!(
        batch["metrics"].as_array().unwrap().len(),
        attempted as usize
    );

    let mut live = LiveSmr::new(&spec).expect("session");
    let mut failed = None;
    for _ in 0..200 {
        let status: serde_json::Value = serde_json::from_str(&live.step(0.1)).unwrap();
        if !status["failure"].is_null() {
            failed = Some(status);
            break;
        }
    }
    let failed = failed.expect("typed boundary failure");
    assert_eq!(failed["round"], attempted);
    assert_eq!(failed["failure"], batch["failure"]);
    assert_eq!(failed["failure"]["node"], 1);
    assert_eq!(failed["failure"]["violation"]["first_mismatch"], 33);
    assert_eq!(batch_report(&live.export_scenario()), live_report(&live));

    let replay_before = live.export_scenario();
    let report_before = live.report_json();
    let repeated: serde_json::Value = serde_json::from_str(&live.step(0.6)).unwrap();
    assert_eq!(repeated["absorbed"], true);
    assert_eq!(repeated["failure"], failed["failure"]);
    assert_eq!(live.export_scenario(), replay_before);
    assert_eq!(live.report_json(), report_before);

    for guarded in [
        live.inject(9, 99, None),
        live.set_traffic("[1.0]"),
        live.set_block(3, true),
        live.node_detail(0),
        live.certs_json(),
        live.chain_json(0),
        live.merkle_tree_json(0, 0),
        live.capture_stale(1),
        live.verify_json(),
    ] {
        let value: serde_json::Value = serde_json::from_str(&guarded).expect("guard json");
        assert!(value["error"].is_string(), "guarded call returned {value}");
    }
    assert_eq!(live.export_scenario(), replay_before);
    assert_eq!(live.report_json(), report_before);
}

// --- Oracle 1: every live lever still replays from the exported spec ---

#[test]
fn recovery_sessions_replay_as_batch_with_every_live_lever() {
    for sticky in [false, true] {
        let mut live = LiveSmr::new_with_mode(&recovery_spec(10), sticky).expect("session");
        for _ in 0..5 {
            live.step(0.1);
        }
        live.inject(5, 99, None);
        live.set_block(2, true);
        for _ in 0..6 {
            live.step(0.4);
        }
        live.set_traffic("[0.5,0.5]");
        live.inject(0, 77, Some(3));
        for _ in 0..5 {
            live.step(0.0);
        }
        live.set_block(2, false);
        live.set_block(9, true);
        for _ in 0..12 {
            live.step(0.05);
        }
        assert_eq!(
            batch_report(&live.export_scenario()),
            live_report(&live),
            "sticky={sticky}"
        );
    }
}

#[test]
fn interleaved_chain_reads_leave_the_replay_identity_intact() {
    // The chain panel polls every round. A read that touched the stream would
    // desync the export, so the O1 shape runs again with chain_json wedged
    // between every lever.
    for sticky in [false, true] {
        let mut live = LiveSmr::new_with_mode(&recovery_spec(10), sticky).expect("session");
        for _ in 0..5 {
            live.step(0.1);
            live.chain_json(0);
        }
        live.inject(5, 99, None);
        live.chain_json(2);
        live.set_block(2, true);
        for _ in 0..6 {
            live.step(0.4);
            live.chain_json(1);
        }
        live.set_traffic("[0.5,0.5]");
        live.inject(0, 77, Some(3));
        for _ in 0..5 {
            live.step(0.0);
            live.chain_json(0);
        }
        live.set_block(2, false);
        live.set_block(9, true);
        for _ in 0..12 {
            live.step(0.05);
            live.chain_json(7);
        }
        assert_eq!(
            batch_report(&live.export_scenario()),
            live_report(&live),
            "sticky={sticky}"
        );
    }
}

// --- Oracle 2: the rec-row arc, frozen ---

/// One frozen row, in the order the arc reads: R composition, window, the
/// cumulative rollback count, and the four boundary latches.
#[expect(clippy::too_many_arguments)]
fn row(
    noreset: u32,
    reset: u32,
    bot_r: u32,
    window: u64,
    rollbacks: u32,
    max_checkpoint_window: u64,
    rollback_depth: u32,
    cp_fork_k: u32,
    max_cp_p_len: u32,
) -> serde_json::Value {
    serde_json::json!({
        "noreset": noreset, "reset": reset, "bot_r": bot_r, "window": window,
        "rollbacks": rollbacks, "max_checkpoint_window": max_checkpoint_window,
        "rollback_depth": rollback_depth, "cp_fork_k": cp_fork_k,
        "max_cp_p_len": max_cp_p_len,
    })
}

#[test]
fn the_rec_row_arc_is_frozen() {
    // n = 8, T = 4, blackout 6–9 (after the first boundary, so the population
    // carries a real checkpoint into it). The arc: healthy → boundary-4 mint
    // → blackout ⊥ → boundary-12 re-arm to Reset → boundary-16 rollback three
    // windows deep, which revives every log → clean again. Boundary 20's latch
    // has no row to land on: the horizon truncates it.
    // Regenerate only as a deliberate, commit-noted act.
    let spec = r#"{"n":8,"seed":5,"k":6,"ell":3,"sigma":2.0,
        "proto":{"kind":"recovery","t_window_rounds":4,"resend_until_acked":false},
        "injections":[{"round":1,"client":1,"op":7}],"max_rounds":20,
        "schedule":{"kind":"windows","windows":[
            {"start_round":6,"rounds":4,"target":{"kind":"sample_fraction","fraction":1.0}}]}}"#;
    let report = batch_report(spec);
    let expected = serde_json::json!([
        row(8, 0, 0, 0, 0, 0, 0, 0, 0),
        row(8, 0, 0, 0, 0, 0, 0, 0, 0),
        row(8, 0, 0, 0, 0, 0, 0, 0, 0),
        row(8, 0, 0, 0, 0, 0, 0, 0, 0),
        row(8, 0, 0, 1, 0, 1, 0, 1, 1),
        row(0, 0, 8, 1, 0, 1, 0, 0, 0),
        row(0, 0, 8, 1, 0, 1, 0, 0, 0),
        row(0, 0, 8, 1, 0, 1, 0, 0, 0),
        row(0, 0, 8, 2, 0, 1, 0, 1, 1),
        row(0, 0, 8, 2, 0, 1, 0, 0, 0),
        row(0, 0, 8, 2, 0, 1, 0, 0, 0),
        row(0, 0, 8, 2, 0, 1, 0, 0, 0),
        row(0, 8, 0, 3, 0, 1, 0, 1, 1),
        row(0, 8, 0, 3, 0, 1, 0, 0, 0),
        row(0, 8, 0, 3, 0, 1, 0, 0, 0),
        row(0, 8, 0, 3, 0, 1, 0, 0, 0),
        row(8, 0, 0, 4, 8, 4, 3, 1, 0),
        row(8, 0, 0, 4, 0, 4, 0, 0, 0),
        row(8, 0, 0, 4, 0, 4, 0, 0, 0),
        row(8, 0, 0, 4, 0, 4, 0, 0, 0),
    ]);
    let mut rows = report["recovery"]["rounds"].clone();
    let agree: Vec<serde_json::Value> = rows
        .as_array_mut()
        .expect("rows")
        .iter_mut()
        .map(|r| {
            r.as_object_mut()
                .expect("row")
                .remove("useful_agree")
                .expect("present")
        })
        .collect();
    assert_eq!(rows, expected);
    // Def 1.4 at each round's end: no useful server from the blackout until the rollback at 16.
    let t = serde_json::Value::Bool(true);
    let none = serde_json::Value::Null;
    let want: Vec<serde_json::Value> = [vec![t.clone(); 5], vec![none; 10], vec![t; 5]].concat();
    assert_eq!(agree, want);
    assert_eq!(report["recovery"]["fork_ok"], true);
    assert_eq!(report["safety_ok"], true);
}

// --- Oracle 3: the other two rules stream exactly what they streamed ---

#[test]
fn extended_and_compact_payloads_never_grow_a_recovery_field() {
    for proto in [
        r#"{"kind":"extended"}"#,
        r#"{"kind":"compact","t_commit_rounds":6}"#,
    ] {
        let spec = format!(
            r#"{{"n":16,"seed":4,"k":6,"ell":3,"sigma":1.5,"proto":{proto},
                "injections":[{{"round":2,"client":1,"op":7}}],"max_rounds":30,
                "schedule":{{"kind":"fresh_per_round","fraction":0.1}}}}"#
        );
        let mut live = LiveSmr::new(&spec).expect("session");
        let mut payloads: Vec<String> = (0..12).map(|_| live.step(0.1)).collect();
        payloads.push(live.report_json());
        payloads.push(live.export_scenario());
        payloads.push(run_smr_json(&spec));
        for payload in &payloads {
            for forbidden in ["\"rec\"", "\"r\":", "\"recovery\"", "manual_blocks"] {
                assert!(
                    !payload.contains(forbidden),
                    "{proto}: {forbidden} leaked into a payload"
                );
            }
        }
    }
}

// --- Oracle 4: the lab's r*̂ scan, twinned in Rust, plus the release law ---

/// The lab's scan re-implemented over the streamed report exactly as smr.js
/// reads it. Changing the predicate here means changing it there.
fn js_r_star(report: &serde_json::Value, n: usize, t_window: usize) -> Option<usize> {
    let rows = report["recovery"]["rounds"].as_array()?;
    let metrics = report["metrics"].as_array()?;
    let floor = (3 * n).div_ceil(4) as u64;
    let boundaries: Vec<usize> = (1..=metrics.len()).filter(|r| r % t_window == 0).collect();
    let clean = |b: usize| {
        rows[b - 1]["reset"].as_u64().expect("reset") == 0
            && metrics[b - 1]["nonbot_logs"].as_u64().expect("nonbot") >= floor
    };
    boundaries.iter().copied().find(|&r_star| {
        let m = &metrics[r_star - 1];
        m["useful_executed"]
            .as_array()
            .is_some_and(|range| range[0] == range[1])
            && boundaries.iter().all(|&b| b < r_star || clean(b))
    })
}

#[test]
fn the_lab_r_star_scan_agrees_with_the_engine_across_seeds() {
    for seed in 0..12u64 {
        for (t_window, blackout_start) in [(10, 12), (10, 25), (20, 30)] {
            let spec = format!(
                r#"{{"n":64,"seed":{seed},"k":6,"ell":3,"sigma":2.0,
                    "proto":{{"kind":"recovery","t_window_rounds":{t_window},"resend_until_acked":false}},
                    "injections":[{{"round":2,"client":1,"op":7}}],"max_rounds":140,
                    "schedule":{{"kind":"windows","windows":[
                        {{"start_round":{blackout_start},"rounds":16,
                          "target":{{"kind":"sample_fraction","fraction":1.0}}}}]}}}}"#
            );
            let report = batch(&spec);
            let value = serde_json::to_value(&report).expect("report serializes");
            assert_eq!(
                js_r_star(&value, 64, t_window),
                report.recovered_round(64, t_window as u64),
                "seed {seed} T {t_window} blackout {blackout_start}"
            );
        }
    }
}

#[test]
fn a_clean_release_recovers_exactly_two_windows_past_the_next_boundary() {
    // The release law R = 2T + (T − φ) mod T, with φ the release round's
    // phase within its window. The mechanism behind it: the first boundary
    // after release re-arms the ⊥ population to Reset, the next one rolls it
    // back onto live logs, and the one after that is the first clean row.
    const T: usize = 10;
    let spec = r#"{"n":64,"seed":17,"k":6,"ell":3,"sigma":2.0,
        "proto":{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false},
        "injections":[{"round":2,"client":1,"op":7}],"max_rounds":70,
        "schedule":{"kind":"windows","windows":[
            {"start_round":12,"rounds":16,"target":{"kind":"sample_fraction","fraction":1.0}}]}}"#;
    let report = batch(spec);
    let release = 12 + 16;
    let phi = release % T;
    let predicted = release + 2 * T + (T - phi) % T;
    assert_eq!(predicted, 50, "the law's arithmetic, spelled out");
    assert_eq!(report.recovered_round(64, T as u64), Some(predicted));

    let value = serde_json::to_value(&report).expect("report serializes");
    assert_eq!(js_r_star(&value, 64, T), Some(predicted), "law card inputs");
}

#[test]
fn merkle_tree_tops_equal_the_drawer_peak_roots_with_and_without_certificates() {
    let compact = recovery_spec(20).replace(
        r#""kind":"recovery","t_window_rounds":20,"resend_until_acked":false"#,
        r#""kind":"compact","t_commit_rounds":20"#,
    );
    let rec = |certs: bool| {
        recovery_spec(20).replace(
            r#""resend_until_acked":false"#,
            &format!(r#""resend_until_acked":{certs}"#),
        )
    };
    for (spec, certs) in [(rec(false), false), (rec(true), true), (compact, true)] {
        let mut live = LiveSmr::new(&spec).expect("live session");
        for _ in 0..200 {
            live.step(0.1);
        }
        let peaks = if certs {
            let snap: serde_json::Value = serde_json::from_str(&live.certs_json()).unwrap();
            let best = snap["committed_server"].as_u64().unwrap() as usize;
            snap["servers"][best]["peaks"].clone()
        } else {
            let snap: serde_json::Value = serde_json::from_str(&live.chain_json(0)).unwrap();
            snap["peaks"].clone()
        };
        let peaks = peaks.as_array().expect("peaks");
        assert!(
            peaks.len() > 1,
            "certs={certs}: want several trees, got {}",
            peaks.len()
        );
        for peak in peaks {
            let height = peak["height"].as_u64().unwrap() as u32;
            let tree: serde_json::Value = serde_json::from_str(
                &live.merkle_tree_json(peak["start"].as_u64().unwrap(), height),
            )
            .unwrap();
            let levels = tree["levels"].as_array().expect("levels");
            assert_eq!(levels.len(), height as usize + 1);
            assert_eq!(
                levels[height as usize][0], peak["root_hex"],
                "certs={certs}"
            );
        }
    }
}
