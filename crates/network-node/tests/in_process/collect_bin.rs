//! `bin/collect.rs` exit codes: every outcome writes the full result set before it exits.

use network_node::harness;
use network_node::record::RoundRecord;
use network_node::spec::NodeRunSpec;
use std::path::Path;
use std::process::{Command, Output};

const SPEC: &str = r#"{"scenario":{"smr":{"n":4,"seed":3,"k":6,"ell":3,"sigma":1.0,
    "proto":{"kind":"compact","t_commit_rounds":3},"injections":[],"max_rounds":4}},
    "sync":"timer"}"#;

fn write_host(hosts: &Path, name: &str, stdout: &str) {
    let dir = hosts.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.1000.stdout")), stdout).unwrap();
}

/// Lays out a Shadow-style outdir from a perfect-network run plus the given client stdout.
fn run_collect(tag: &str, client_stdout: &str) -> (Output, std::path::PathBuf) {
    run_collect_with(tag, client_stdout, |_| {})
}

fn run_collect_with(
    tag: &str,
    client_stdout: &str,
    tweak: impl FnOnce(&mut Vec<Vec<RoundRecord>>),
) -> (Output, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("collect-bin-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let spec = NodeRunSpec::from_json(SPEC).expect("spec");
    let hosts = dir.join("shadow").join("hosts");
    let mut records = harness::run(&spec);
    tweak(&mut records);
    for (i, recs) in records.iter().enumerate() {
        let lines: Vec<String> = recs.iter().map(|r| r.to_jsonl()).collect();
        write_host(&hosts, &format!("peer{i}"), &lines.join("\n"));
    }
    write_host(&hosts, "client", client_stdout);
    std::fs::write(dir.join("spec.json"), SPEC).unwrap();
    let out = dir.join("out");
    let output = Command::new(env!("CARGO_BIN_EXE_collect"))
        .args([
            "--outdir",
            dir.join("shadow").to_str().unwrap(),
            "--spec",
            dir.join("spec.json").to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("spawn collect");
    (output, out)
}

fn client_stdout(lost: u32, starved: u32) -> String {
    let mut lines = Vec::new();
    for r in 1..=4 {
        let (l, s) = if r == 2 { (lost, starved) } else { (0, 0) };
        lines.push(format!("{{\"r\":{r},\"arrivals\":0}}"));
        lines.push(format!("{{\"r\":{r},\"sent\":0}}"));
        lines.push(format!("{{\"r\":{r},\"lost\":{l}}}"));
        lines.push(format!("{{\"r\":{r},\"starved\":{s}}}"));
    }
    lines.push(r#"{"commands":[]}"#.to_string());
    lines.join("\n")
}

fn assert_outputs_written(out: &Path) -> String {
    for f in [
        "metrics.csv",
        "messages.csv",
        "summary.json",
        "commands.json",
    ] {
        assert!(out.join(f).exists(), "{f} missing");
    }
    std::fs::read_to_string(out.join("summary.json")).unwrap()
}

#[test]
fn a_clean_client_exits_zero() {
    let (output, out) = run_collect("clean", &client_stdout(0, 0));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let summary = assert_outputs_written(&out);
    assert!(summary.contains("\"client_lost\":0,\"client_starved\":0"));
}

#[test]
fn lost_client_attempts_exit_one_after_writing_every_output() {
    let (output, out) = run_collect("lost", &client_stdout(2, 0));
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let summary = assert_outputs_written(&out);
    assert!(summary.contains("\"client_lost\":2"), "{summary}");
}

#[test]
fn starved_client_attempts_exit_one_after_writing_every_output() {
    let (output, out) = run_collect("starved", &client_stdout(0, 3));
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let summary = assert_outputs_written(&out);
    assert!(summary.contains("\"client_starved\":3"), "{summary}");
}

#[test]
fn an_sn_order_violation_exits_three_with_every_output_written() {
    let (output, out) = run_collect_with("sn", &client_stdout(0, 0), |records| {
        for rec in &mut records[1][1..] {
            rec.sn_order_violation = true;
        }
    });
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let summary = assert_outputs_written(&out);
    assert!(summary.contains("\"safety_ok\":false"), "{summary}");
}
