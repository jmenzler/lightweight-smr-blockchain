//! Ledger provenance and artifact naming. Single-file binary: one test changes the process cwd.

use sim::{BlockSchedule, Config, Init, Outcome, Scenario, run_recorded};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

static CWD_LOCK: Mutex<()> = Mutex::new(());

fn scenario(seed: u64, beta: f64) -> Scenario {
    Scenario {
        n: 40,
        seed,
        cfg: Config::default(),
        init: Init::Split { fraction: 0.5 },
        max_rounds: 60,
        schedule: BlockSchedule::FreshPerRound { fraction: beta },
        partition: None,
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sim-prov-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rows(ledger: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(ledger)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn ledger_commit_is_the_build_commit_not_the_working_directory_head() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("cwd");
    git(&dir, &["init", "-q"]);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "x"]);
    let original = std::env::current_dir().unwrap();
    let ledger = dir.join("ledger.jsonl");
    let outcome = Outcome::NoConvergence { rounds: 10 };

    std::env::set_current_dir(&dir).unwrap();
    let first_cwd = git(&dir, &["rev-parse", "HEAD"]);
    sim::log_run_to(&ledger, &scenario(1, 0.0), &outcome, None).unwrap();
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "y"]);
    sim::log_run_to(&ledger, &scenario(1, 0.0), &outcome, None).unwrap();
    let second_cwd = git(&dir, &["rev-parse", "HEAD"]);
    std::env::set_current_dir(original).unwrap();

    let rows = rows(&ledger);
    assert_eq!(
        rows[0]["commit"], rows[1]["commit"],
        "the same binary wrote both rows"
    );
    assert_ne!(rows[0]["commit"].as_str().unwrap(), first_cwd);
    assert_eq!(rows[0]["cwd_commit"], first_cwd.as_str());
    assert_eq!(rows[1]["cwd_commit"], second_cwd.as_str());
    assert!(rows[0]["cwd_dirty"].is_boolean());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ledger_commit_matches_the_repository_head_the_binary_was_built_at() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tmp("head");
    let ledger = dir.join("ledger.jsonl");
    sim::log_run_to(
        &ledger,
        &scenario(1, 0.0),
        &Outcome::NoConvergence { rounds: 1 },
        None,
    )
    .unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        rows(&ledger)[0]["commit"],
        git(repo, &["rev-parse", "HEAD"]).as_str()
    );
    std::fs::remove_dir_all(&dir).ok();
}
#[test]
fn same_second_runs_with_equal_n_and_seed_get_distinct_artifacts() {
    let dir = tmp("names");
    let ledger = dir.join("ledger.jsonl");
    let traces = dir.join("traces");
    let mut paths = Vec::new();
    for beta in [0.05, 0.15, 0.25, 0.35] {
        let (_, metrics) = run_recorded(&scenario(7, beta), &ledger, &traces, 1_000_000, None);
        paths.push(metrics);
    }
    let unique: std::collections::BTreeSet<_> = paths.iter().collect();
    assert_eq!(
        unique.len(),
        paths.len(),
        "metrics paths collide: {paths:?}"
    );
    let traces_listed = std::fs::read_dir(&traces).unwrap().count();
    assert_eq!(traces_listed, 4, "one trace per run");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn identical_reruns_never_overwrite_an_existing_artifact() {
    let dir = tmp("rerun");
    let ledger = dir.join("ledger.jsonl");
    let traces = dir.join("traces");
    let mut metrics = Vec::new();
    for _ in 0..3 {
        let (_, m) = run_recorded(&scenario(7, 0.1), &ledger, &traces, 1_000_000, None);
        metrics.push(m);
    }
    let unique: std::collections::BTreeSet<_> = metrics.iter().collect();
    assert_eq!(unique.len(), 3, "reruns share a metrics file: {metrics:?}");
    for row in rows(&ledger) {
        let path = row["metrics_path"].as_str().unwrap();
        assert!(Path::new(path).exists());
    }
    std::fs::remove_dir_all(&dir).ok();
}
