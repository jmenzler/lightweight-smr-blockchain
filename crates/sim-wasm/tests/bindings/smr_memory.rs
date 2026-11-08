//! The lab's memory view: per-node standalone bytes, the simulator's shared node bytes and their
//! ratio. A read-only getter, so a session that calls it every round still replays as batch.

use crate::batch_view::live_report_as_batch;
use sim_wasm::{LiveSmr, run_smr_json};

fn spec(proto: &str) -> String {
    format!(
        r#"{{"n":32,"seed":11,"k":6,"ell":3,"sigma":1.5,
            "proto":{proto},
            "injections":[{{"round":2,"client":1,"op":7}}],
            "max_rounds":200,
            "schedule":{{"kind":"fresh_per_round","fraction":0.1}},
            "traffic":{{"kind":"pmf","arrivals_pmf":[0.0,1.0]}}}}"#
    )
}

fn memory(live: &LiveSmr) -> serde_json::Value {
    serde_json::from_str(&live.memory_json()).expect("memory json")
}

#[test]
fn memory_reports_per_node_and_simulator_bytes() {
    for proto in [
        r#"{"kind":"compact","t_commit_rounds":12}"#,
        r#"{"kind":"recovery","t_window_rounds":24,"resend_until_acked":true}"#,
    ] {
        let mut live = LiveSmr::new(&spec(proto)).unwrap();
        for _ in 0..80 {
            live.step(0.1);
        }
        let m = memory(&live);
        assert!(m["error"].is_null(), "{proto}: {m}");
        assert_eq!(m["round"], 80);
        assert_eq!(m["n"], 32);
        let counted = m["counted"].as_u64().unwrap();
        assert!(counted > 0 && counted <= 32);
        let (median, max) = (m["median"].as_u64().unwrap(), m["max"].as_u64().unwrap());
        assert!(median > 0 && median <= max, "{proto}: {m}");
        let node = &m["median_node"];
        let sum: u64 = ["log", "executed", "sn", "checkpoint", "certs"]
            .iter()
            .map(|k| node[k].as_u64().unwrap())
            .sum();
        assert_eq!(sum, median, "{proto}: the median node's categories add up");
        assert!(
            node["certs"].as_u64().unwrap() > 0,
            "{proto}: the §5 state is charged to the node, wherever the lab keeps it"
        );
        let shared = m["simulator_node_bytes"].as_u64().unwrap();
        assert!(shared > 0);
        let all = m["standalone_bytes"].as_u64().unwrap();
        assert!(
            all >= median * counted / 2,
            "{proto}: all servers apart hold at least half the median each"
        );
        let sharing = m["sharing"].as_f64().unwrap();
        assert!(
            (sharing - all as f64 / shared as f64).abs() < 1e-9,
            "{proto}: sharing = all n servers apart ÷ the simulator's bytes for the same n servers"
        );
        assert!(sharing >= 1.0, "{proto}: {m}");
        assert!(
            m["linear_bytes"].is_null(),
            "native builds have no wasm linear memory"
        );
    }
}

#[test]
fn extended_has_no_memory_view() {
    let mut live = LiveSmr::new(&spec(r#"{"kind":"extended"}"#)).unwrap();
    live.step(0.0);
    assert!(memory(&live)["error"].is_string());
}

#[test]
fn reading_memory_every_round_leaves_the_session_replayable() {
    for proto in [
        r#"{"kind":"compact","t_commit_rounds":12}"#,
        r#"{"kind":"recovery","t_window_rounds":24,"resend_until_acked":true}"#,
    ] {
        let mut live = LiveSmr::new(&spec(proto)).unwrap();
        for _ in 0..50 {
            live.step(0.1);
            memory(&live);
        }
        let out: serde_json::Value =
            serde_json::from_str(&run_smr_json(&live.export_scenario())).unwrap();
        assert_eq!(out["report"], live_report_as_batch(&live), "{proto}");
    }
}
