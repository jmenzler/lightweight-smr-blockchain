use sim::{Availability, BlockSchedule, Init, Outcome, Scenario, log_run_to};
use std::fs;

fn scenario(seed: u64) -> Scenario {
    Scenario {
        n: 100,
        seed,
        cfg: protocol::Config::default(),
        init: Init::Split { fraction: 0.5 },
        max_rounds: 200,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        partition: None,
    }
}

fn tmp_log(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("sim-runlog-test-{name}.jsonl"));
    let _ = fs::remove_file(&p);
    p
}

#[test]
fn record_carries_commit_params_and_outcome() {
    let path = tmp_log("fields");
    let outcome = Outcome::Agreement {
        value: 1,
        rounds: 17,
        availability: Availability::Held,
    };
    log_run_to(&path, &scenario(42), &outcome, None).unwrap();
    let line = fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();

    assert!(
        v["commit"].as_str().unwrap().len() >= 7,
        "commit hash recorded"
    );
    assert!(v["dirty"].is_boolean(), "dirty flag recorded");
    assert!(v["timestamp"].is_string());
    assert_eq!(v["scenario"]["n"], 100);
    assert_eq!(v["scenario"]["seed"], 42);
    assert_eq!(
        v["scenario"]["cfg"]["k"], 6,
        "(k,ℓ) in ledger = regenerable"
    );
    assert_eq!(v["scenario"]["cfg"]["ell"], 3);
    assert_eq!(v["scenario"]["schedule"]["FreshPerRound"]["fraction"], 0.1);
    assert_eq!(v["outcome"]["Agreement"]["rounds"], 17);
}

#[test]
fn runs_append_never_overwrite() {
    let path = tmp_log("append");
    let outcome = Outcome::NoConvergence { rounds: 200 };
    log_run_to(&path, &scenario(1), &outcome, None).unwrap();
    log_run_to(&path, &scenario(2), &outcome, None).unwrap();
    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content.lines().count(), 2);
    let seeds: Vec<u64> = content
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["scenario"]["seed"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert_eq!(seeds, [1, 2]);
}

#[test]
fn trace_is_stored_and_referenced_in_ledger() {
    let path = tmp_log("trace");
    let trace_dir = std::env::temp_dir().join("sim-runlog-test-traces");
    let _ = fs::remove_dir_all(&trace_dir);
    let (outcome, _) = sim::run_recorded(&scenario(9), &path, &trace_dir, 10_000_000, None);
    assert!(matches!(
        outcome,
        Outcome::Agreement { .. } | Outcome::NoConvergence { .. }
    ));

    let v: serde_json::Value =
        serde_json::from_str(fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
    let trace_path = v["trace_path"].as_str().expect("trace path referenced");
    let trace_raw = fs::read_to_string(trace_path).expect("trace file exists at referenced path");
    let trace: serde_json::Value = serde_json::from_str(&trace_raw).unwrap();
    assert_eq!(trace["n"], 100);
    assert_eq!(trace["seed"], 9);
    assert!(!trace["rounds"].as_array().unwrap().is_empty());
}

#[test]
fn two_ledgers_sharing_a_trace_dir_never_collide_on_a_trace_name() {
    let root = std::env::temp_dir().join(format!("sim-runlog-test-shared-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let trace_dir = root.join("traces");
    let last_row = |ledger: &std::path::Path| -> serde_json::Value {
        let text = fs::read_to_string(ledger).unwrap();
        serde_json::from_str(text.lines().last().unwrap()).unwrap()
    };
    let file_name = |p: &str| std::path::Path::new(p).file_name().unwrap().to_owned();
    // Same-second runs share a metrics name in their own dirs, so their trace names coincide.
    // Fresh dirs per attempt: a pair split by a second boundary leaves shared dirs' `-k` suffixes out of step.
    let mut same_second = false;
    for attempt in 0..20 {
        let ledgers = [
            root.join(format!("a{attempt}")).join("ledger.jsonl"),
            root.join(format!("b{attempt}")).join("ledger.jsonl"),
        ];
        for ledger in &ledgers {
            fs::create_dir_all(ledger.parent().unwrap()).unwrap();
            sim::run_recorded(&scenario(21), ledger, &trace_dir, 10_000_000, None);
        }
        let (a, b) = (last_row(&ledgers[0]), last_row(&ledgers[1]));
        let traces = [
            a["trace_path"].as_str().unwrap(),
            b["trace_path"].as_str().unwrap(),
        ];
        assert_ne!(traces[0], traces[1]);
        for t in traces {
            assert!(fs::metadata(t).is_ok(), "{t} exists");
        }
        if file_name(a["metrics_path"].as_str().unwrap())
            == file_name(b["metrics_path"].as_str().unwrap())
        {
            same_second = true;
            break;
        }
    }
    assert!(same_second, "precondition: two runs within one second");
    fs::remove_dir_all(&root).ok();
}

#[test]
fn oversized_run_skips_trace_but_still_logs() {
    let path = tmp_log("trace-cap");
    let trace_dir = std::env::temp_dir().join("sim-runlog-test-traces-cap");
    // cap of 10 cells << n*max_rounds = 20_000 → trace must be skipped
    sim::run_recorded(&scenario(3), &path, &trace_dir, 10, None);

    let v: serde_json::Value =
        serde_json::from_str(fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
    assert!(
        v["trace_path"].is_null(),
        "no trace stored above the cell cap"
    );
    assert_eq!(v["scenario"]["seed"], 3, "run still fully logged");
    assert!(
        v["metrics_path"].is_string(),
        "metrics are tiny — stored even when the trace is over the cap"
    );
}

#[test]
fn metrics_csv_is_stored_and_referenced_in_ledger() {
    let path = tmp_log("metrics");
    let trace_dir = std::env::temp_dir().join("sim-runlog-test-traces-metrics");
    let _ = fs::remove_dir_all(&trace_dir);
    sim::run_recorded(&scenario(5), &path, &trace_dir, 10_000_000, None);

    let v: serde_json::Value =
        serde_json::from_str(fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
    let metrics_path = v["metrics_path"].as_str().expect("metrics path referenced");
    let csv = fs::read_to_string(metrics_path).expect("metrics file exists");
    let mut lines = csv.lines();
    assert_eq!(
        lines.next().unwrap(),
        "round,holders,undecided,blocked,useful,distinct_values"
    );
    let expected_rows = sim::run_report(&scenario(5)).metrics.len();
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), expected_rows, "one row per simulated round");
    assert!(rows[0].starts_with("1,"));
    assert!(rows.iter().all(|r| r.split(',').count() == 6));
}

#[test]
fn runlog_off_returns_plain_outcome_without_touching_the_ledger() {
    let ledger = sim::runlog::default_log_path();
    let len_before = fs::metadata(&ledger).map(|m| m.len()).ok();
    // Env is process-global; this is the only test in this binary reading it.
    unsafe { std::env::set_var("SIM_RUNLOG", "off") };
    let (outcome, _) = sim::run_logged(&scenario(11), None);
    unsafe { std::env::remove_var("SIM_RUNLOG") };

    assert_eq!(
        outcome,
        sim::run(&scenario(11)),
        "opt-out must not change the run"
    );
    let len_after = fs::metadata(&ledger).map(|m| m.len()).ok();
    assert_eq!(
        len_before, len_after,
        "no ledger append under SIM_RUNLOG=off"
    );
}

#[test]
fn zero_trace_cap_skips_trace_dir_but_keeps_outcome() {
    let path = tmp_log("trace-off");
    let trace_dir = std::env::temp_dir().join("sim-runlog-test-traces-off");
    let _ = fs::remove_dir_all(&trace_dir);
    // cap 0 is the SIM_TRACE=off path run_logged resolves to
    let (outcome, _) = sim::run_recorded(&scenario(13), &path, &trace_dir, 0, None);
    assert_eq!(
        outcome,
        sim::run(&scenario(13)),
        "tracing must not change the run"
    );
    assert!(!trace_dir.exists(), "no trace dir created at cap 0");

    let v: serde_json::Value =
        serde_json::from_str(fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
    assert!(v["trace_path"].is_null());
    assert!(v["metrics_path"].is_string(), "metrics still stored");
}

#[test]
fn caller_blob_is_embedded() {
    let path = tmp_log("blob");
    let outcome = Outcome::AllUndecided { rounds: 33 };
    let extra = serde_json::json!({ "sweep": "gray-zone", "arm": "A" });
    log_run_to(&path, &scenario(7), &outcome, Some(extra)).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(v["extra"]["arm"], "A");
}
