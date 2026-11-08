//! Inputs the lab cannot honour are refused loudly instead of being clamped, ignored or trapped.

use crate::batch_view::live_report_as_batch;
use sim_wasm::{LiveSim, LiveSmr, run_scenario_json, run_smr_json};

fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).unwrap()
}

fn smr_spec(proto: &str, extra: &str) -> String {
    format!(
        r#"{{"n":16,"seed":4,"k":6,"ell":3,"sigma":1.5,"proto":{proto},
            "injections":[],"max_rounds":50{extra},
            "schedule":{{"kind":"fresh_per_round","fraction":0.0}}}}"#
    )
}

fn compact_spec(extra: &str) -> String {
    smr_spec(r#"{"kind":"compact","t_commit_rounds":6}"#, extra)
}

const BAD_FRACTIONS: [f64; 4] = [1.5, -0.1, f64::NAN, f64::INFINITY];

#[test]
fn live_sim_refuses_a_fraction_outside_the_unit_interval_and_does_not_advance() {
    let spec =
        r#"{"n":20,"seed":3,"k":6,"ell":3,"init":{"kind":"split","fraction":0.5},"max_rounds":1}"#;
    let mut live = LiveSim::new(spec).unwrap();
    for fraction in BAD_FRACTIONS {
        let out = parse(&live.step(fraction));
        assert!(out["error"].is_string(), "fraction {fraction}: {out}");
    }
    live.step(1.0);
    live.step(0.0);
    let replay = parse(&run_scenario_json(&live.export_scenario()));
    assert!(replay["error"].is_null(), "export must replay: {replay}");
    assert_eq!(replay["trace"], parse(&live.trace_json()));
}

#[test]
fn live_smr_refuses_a_fraction_outside_the_unit_interval_and_does_not_advance() {
    let mut live = LiveSmr::new(&compact_spec("")).unwrap();
    for fraction in BAD_FRACTIONS {
        let out = parse(&live.step(fraction));
        assert!(out["error"].is_string(), "fraction {fraction}: {out}");
    }
    live.step(1.0);
    live.step(0.0);
    let replay = parse(&run_smr_json(&live.export_scenario()));
    assert!(replay["error"].is_null(), "export must replay: {replay}");
    assert_eq!(replay["report"], live_report_as_batch(&live));
}

#[test]
fn a_fraction_that_does_not_survive_json_replays_to_the_same_blocked_count() {
    // 2/13 prints with 17 significant digits; without exact float parsing the replay blocks one server fewer.
    let spec = compact_spec("").replace("\"n\":16", "\"n\":13");
    let mut live = LiveSmr::new(&spec).unwrap();
    for _ in 0..10 {
        live.step(2.0 / 13.0);
    }
    let replay = parse(&run_smr_json(&live.export_scenario()));
    assert_eq!(replay["report"], live_report_as_batch(&live));
}

#[test]
fn recovery_with_skip_is_refused_at_the_lab_entries_instead_of_trapping() {
    let spec = smr_spec(
        r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":true}"#,
        r#","repeated_commit":"skip""#,
    );
    let err = LiveSmr::new(&spec).err().expect("live entry refuses");
    assert!(err.contains("repeated_commit"), "{err}");
    let out = parse(&run_smr_json(&spec));
    assert!(out["error"].is_string(), "{out}");
}

#[test]
fn live_smr_refuses_spec_fields_it_would_otherwise_drop() {
    let blocks = compact_spec(r#","manual_blocks":[{"node":0,"from_round":1}]"#);
    let err = LiveSmr::new(&blocks).err().expect("manual_blocks refused");
    assert!(err.contains("manual_blocks"), "{err}");
    let halt = compact_spec(r#","halt_on_violation":true"#);
    let err = LiveSmr::new(&halt)
        .err()
        .expect("halt_on_violation refused");
    assert!(err.contains("halt_on_violation"), "{err}");
    for spec in [blocks, halt] {
        let out = parse(&run_smr_json(&spec));
        assert!(out["error"].is_null(), "batch import still runs it: {out}");
    }
}

#[test]
fn a_live_inject_that_would_renumber_a_later_scheduled_command_is_refused() {
    let spec = compact_spec("").replace(
        r#""injections":[]"#,
        r#""injections":[{"round":30,"client":1,"op":5}]"#,
    );
    let mut live = LiveSmr::new(&spec).unwrap();
    let out = parse(&live.inject(1, 7, None));
    assert!(out["error"].is_string(), "{out}");
    assert!(
        parse(&live.inject(2, 8, None))["error"].is_null(),
        "another client is unaffected"
    );
    for _ in 0..25 {
        live.step(0.0);
    }
    let replay = parse(&run_smr_json(&live.export_scenario()));
    assert_eq!(replay["report"], live_report_as_batch(&live));
}

#[test]
fn a_sigma_below_one_is_refused_at_both_lab_entries_instead_of_clamped() {
    let spec = compact_spec("").replace("\"sigma\":1.5", "\"sigma\":0.5");
    let err = LiveSmr::new(&spec).err().expect("live entry refuses");
    assert!(err.contains("sigma"), "{err}");
    let out = parse(&run_smr_json(&spec));
    assert!(out["error"].as_str().unwrap().contains("sigma"), "{out}");
}
