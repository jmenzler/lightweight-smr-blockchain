//! Append-only JSONL run ledger: commit + params + outcome per run, so every number is regenerable.
//!
//! `commit`/`dirty` describe the source the binary was built from (protocol + sim crates, build-time
//! state); `cwd_commit`/`cwd_dirty` describe the repository of the working directory at write time.

use crate::smr::{
    Proto, RunFailure, SmrReport, SmrScenario, SmrSummary, SmrTerminal, amp_count, run_smr,
};
use crate::{Outcome, Scenario};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VALUE_METRICS_CSV_HEADER: &str = "round,holders,undecided,blocked,useful,distinct_values";
pub const SMR_METRICS_CSV_HEADER: &str = "round,nonbot_logs,blocked,useful,distinct_logs,max_log_len,min_executed_len,max_executed_len,arrivals,lcp_len";

pub const BUILD_COMMIT: &str = env!("SIM_BUILD_COMMIT");

pub const DEFAULT_LOG: &str = ".runs/sim-runs.jsonl";

#[derive(Serialize)]
struct RunRecord<'a> {
    timestamp: String,
    commit: String,
    dirty: bool,
    cwd_commit: String,
    cwd_dirty: bool,
    scenario: &'a Scenario,
    outcome: &'a Outcome,
    trace_path: Option<String>,
    metrics_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<serde_json::Value>,
}

pub const DEFAULT_TRACE_DIR: &str = ".runs/traces";
pub const DEFAULT_TRACE_CAP_CELLS: u64 = 20_000_000;

fn git(args: &[&str]) -> Option<String> {
    // `status` would otherwise take index.lock and fail a concurrent `git commit` in that repo.
    let out = Command::new("git")
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// FNV-1a over the serialized parameters: a stable tag, so runs differing only in parameters never share a name.
pub(crate) fn params_tag<S: Serialize>(params: &S) -> String {
    let json = serde_json::to_string(params).expect("params serialize");
    let hash = json.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    format!("{:08x}", hash >> 32)
}

const MAX_NAME_ATTEMPTS: u32 = 1000;

/// Creates a file that did not exist, appending `-1`, `-2`, ... to `stem` on collision.
pub(crate) fn create_new_unique(
    dir: &Path,
    stem: &str,
    ext: &str,
) -> Result<(std::fs::File, PathBuf), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("artifact dir {}: {e}", dir.display()))?;
    for attempt in 0..MAX_NAME_ATTEMPTS {
        let name = if attempt == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{attempt}.{ext}")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("artifact create failed ({}): {e}", path.display())),
        }
    }
    Err(format!(
        "no free artifact name for {stem}.{ext} in {}",
        dir.display()
    ))
}

/// Writes `contents` to a file that did not exist, appending `-1`, `-2`, ... to `stem` on collision.
fn write_new_unique(dir: &Path, stem: &str, ext: &str, contents: &str) -> PathBuf {
    let (mut f, path) = create_new_unique(dir, stem, ext).unwrap_or_else(|e| panic!("{e}"));
    f.write_all(contents.as_bytes())
        .unwrap_or_else(|e| panic!("artifact write failed ({}): {e}", path.display()));
    path
}

/// Writes `contents` to `path`, which must not exist.
fn write_new_exact(path: &Path, contents: &str) {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap_or_else(|e| panic!("artifact create failed ({}): {e}", path.display()));
    f.write_all(contents.as_bytes())
        .unwrap_or_else(|e| panic!("artifact write failed ({}): {e}", path.display()));
}

/// The artifact file name shared by a run's files: second, size, seed and a tag of the full parameters.
fn artifact_stem<S: Serialize>(prefix: &str, n: usize, seed: u64, params: &S) -> String {
    tagged_artifact_stem(prefix, n, seed, &params_tag(params))
}

/// [`artifact_stem`] from an already computed parameter tag.
pub(crate) fn tagged_artifact_stem(prefix: &str, n: usize, seed: u64, tag: &str) -> String {
    let stamp = utc_now().replace(':', "");
    format!("{stamp}-{prefix}n{n}-seed{seed}-{tag}")
}

pub(crate) fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // civil-from-days (Hinnant): valid for the unix era
    let z = days as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

pub fn log_run_to(
    path: &Path,
    scenario: &Scenario,
    outcome: &Outcome,
    extra: Option<serde_json::Value>,
) -> std::io::Result<()> {
    log_run_record(path, scenario, outcome, None, None, extra)
}

/// (timestamp, build commit, build-time source dirtiness).
pub(crate) fn provenance() -> (String, String, bool) {
    (
        utc_now(),
        BUILD_COMMIT.to_string(),
        env!("SIM_BUILD_DIRTY") != "false",
    )
}

/// Commit and dirtiness of the repository the process runs in; may differ from the build.
fn cwd_provenance() -> (String, bool) {
    (
        git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into()),
        git(&["status", "--porcelain"]).is_none_or(|s| !s.is_empty()),
    )
}

fn log_run_record(
    path: &Path,
    scenario: &Scenario,
    outcome: &Outcome,
    trace_path: Option<String>,
    metrics_path: Option<String>,
    extra: Option<serde_json::Value>,
) -> std::io::Result<()> {
    let (timestamp, commit, dirty) = provenance();
    let (cwd_commit, cwd_dirty) = cwd_provenance();
    let record = RunRecord {
        timestamp,
        commit,
        dirty,
        cwd_commit,
        cwd_dirty,
        scenario,
        outcome,
        trace_path,
        metrics_path,
        extra,
    };
    append_jsonl(path, &record)
}

fn append_jsonl<S: Serialize>(path: &Path, record: &S) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(
        f,
        "{}",
        serde_json::to_string(record).expect("record serializes")
    )?;
    Ok(())
}

pub fn default_log_path() -> PathBuf {
    git(&["rev-parse", "--show-toplevel"])
        .map(|top| Path::new(&top).join(DEFAULT_LOG))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LOG))
}

/// Ledger path: an explicit path always logs; the repo-root default honors SIM_RUNLOG=off.
pub fn ledger_target(explicit: Option<&Path>) -> Option<PathBuf> {
    match explicit {
        Some(p) => Some(p.to_path_buf()),
        None if std::env::var("SIM_RUNLOG").as_deref() == Ok("off") => None,
        None => Some(default_log_path()),
    }
}

/// `run()` + ledger append, returning the metrics CSV path; a ledger write failure panics, since an unlogged run is unauditable.
pub fn run_logged(
    scenario: &Scenario,
    extra: Option<serde_json::Value>,
) -> (Outcome, Option<String>) {
    let Some(ledger) = ledger_target(None) else {
        return (crate::run(scenario), None);
    };
    let cap = if std::env::var("SIM_TRACE").as_deref() == Ok("off") {
        0
    } else {
        std::env::var("SIM_TRACE_MAX_CELLS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_TRACE_CAP_CELLS)
    };
    let trace_dir = ledger.parent().unwrap_or(Path::new(".")).join("traces");
    let (outcome, metrics_path) = run_recorded(scenario, &ledger, &trace_dir, cap, extra);
    (outcome, Some(metrics_path))
}

pub fn run_recorded(
    scenario: &Scenario,
    ledger: &Path,
    trace_dir: &Path,
    cap_cells: u64,
    extra: Option<serde_json::Value>,
) -> (Outcome, String) {
    let stem = artifact_stem("", scenario.n, scenario.seed, scenario);
    let cells = scenario.n as u64 * scenario.max_rounds as u64;
    let record_trace = cells <= cap_cells && cap_cells > 0;
    let (outcome, metrics, trace) = crate::run_inner(scenario, record_trace);

    let mut csv = format!("{VALUE_METRICS_CSV_HEADER}\n");
    for (i, m) in metrics.iter().enumerate() {
        csv.push_str(&format!(
            "{},{},{},{},{},{}\n",
            i + 1,
            m.holders,
            m.undecided,
            m.blocked,
            m.useful,
            m.distinct_values
        ));
    }
    let metrics_dir = ledger.parent().unwrap_or(Path::new(".")).join("metrics");
    let metrics_file = write_new_unique(&metrics_dir, &stem, "csv", &csv);
    let metrics_path = metrics_file.to_string_lossy().into_owned();
    let run_stem = metrics_file
        .file_stem()
        .expect("artifact has a stem")
        .to_string_lossy()
        .into_owned();

    // The trace dir can be shared by ledgers whose metrics dirs differ, so the name is reserved anew.
    let trace_path = trace.map(|t| {
        let json = serde_json::to_string(&t).expect("trace serializes");
        write_new_unique(trace_dir, &run_stem, "json", &json)
            .to_string_lossy()
            .into_owned()
    });

    log_run_record(
        ledger,
        scenario,
        &outcome,
        trace_path,
        Some(metrics_path.clone()),
        extra,
    )
    .expect("run ledger write failed (set SIM_RUNLOG=off to opt out)");
    (outcome, metrics_path)
}

#[derive(Serialize)]
struct SmrOutcomeSummary {
    terminal: SmrTerminal,
    /// No fork across all servers; Def 1.4 and Def 1.5 are the `committed` block.
    safety_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    exec_dup_round: Option<usize>,
    /// Absent for extended, which executes nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    committed: Option<CommittedSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repeat_skips: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_repeat_skip_round: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    boundary_skip_events: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_boundary_skip_round: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_boundary_skip_round: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_rejoin_rounds: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    longest_stale_rounds: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unrejoined_nodes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    horizon_skip_nodes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wedged_round: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<RunFailure>,
    commands: usize,
    complete: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_fork_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_commit_fork_round: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool_peak_in_flight: Option<u32>,
}

/// The report's Def 1.5 retraction and Def 1.4 strong-safety counters, zeros included.
#[derive(Serialize)]
struct CommittedSummary {
    retractions: u64,
    retraction_rewrites: u64,
    first_retraction_round: Option<usize>,
    strong_safety_violation_rounds: u64,
    strong_safety_first_violation: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    strong_safety_boundary_violations: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    strong_safety_boundary_first_violation: Option<usize>,
}

#[derive(Serialize)]
struct SmrRunRecord<'a, O> {
    timestamp: String,
    commit: String,
    dirty: bool,
    cwd_commit: String,
    cwd_dirty: bool,
    scenario: &'a SmrScenario,
    amp_count: usize,
    outcome: O,
    trace_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commands_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_path: Option<String>,
    // Read off the report so the ledger cannot claim a spill that was not written.
    #[serde(skip_serializing_if = "Option::is_none")]
    spill_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra: Option<serde_json::Value>,
}

fn smr_outcome_summary(scenario: &SmrScenario, report: &SmrSummary) -> SmrOutcomeSummary {
    let recovery = matches!(scenario.proto, Proto::Recovery { .. });
    SmrOutcomeSummary {
        terminal: report.terminal,
        safety_ok: report.safety_ok,
        exec_dup_round: report.exec_dup_round,
        committed: (!matches!(scenario.proto, Proto::Extended)).then(|| CommittedSummary {
            retractions: report.retractions,
            retraction_rewrites: report.retraction_rewrites,
            first_retraction_round: report.first_retraction_round,
            strong_safety_violation_rounds: report.strong_safety_violation_rounds,
            strong_safety_first_violation: report.strong_safety_first_violation,
            strong_safety_boundary_violations: recovery
                .then_some(report.strong_safety_boundary_violations),
            strong_safety_boundary_first_violation: report.strong_safety_boundary_first_violation,
        }),
        repeat_skips: (report.repeat_skips > 0).then_some(report.repeat_skips),
        first_repeat_skip_round: report.first_repeat_skip_round,
        boundary_skip_events: (report.boundary_skip_events > 0)
            .then_some(report.boundary_skip_events),
        first_boundary_skip_round: report.first_boundary_skip_round,
        last_boundary_skip_round: report.last_boundary_skip_round,
        max_rejoin_rounds: report.max_rejoin_rounds,
        longest_stale_rounds: report.longest_stale_rounds,
        unrejoined_nodes: (report.unrejoined_nodes > 0).then_some(report.unrejoined_nodes),
        horizon_skip_nodes: (report.horizon_skip_nodes > 0).then_some(report.horizon_skip_nodes),
        wedged_round: report.wedged_round,
        failure: report.failure.clone(),
        commands: report.commands.commands,
        complete: report.commands.complete,
        recovery_fork_ok: report.recovery.as_ref().map(|r| r.fork_ok),
        recovery_commit_fork_round: report.recovery.as_ref().and_then(|r| r.commit_fork_round),
        pool_peak_in_flight: report.pool_peak_in_flight,
    }
}

fn smr_run_record<'a>(
    scenario: &'a SmrScenario,
    report: &SmrSummary,
    metrics_path: Option<String>,
    commands_path: Option<String>,
    recovery_path: Option<String>,
    extra: Option<serde_json::Value>,
) -> SmrRunRecord<'a, SmrOutcomeSummary> {
    let (timestamp, commit, dirty) = provenance();
    let (cwd_commit, cwd_dirty) = cwd_provenance();
    SmrRunRecord {
        timestamp,
        commit,
        dirty,
        cwd_commit,
        cwd_dirty,
        scenario,
        amp_count: amp_count(scenario.sigma, scenario.n),
        outcome: smr_outcome_summary(scenario, report),
        trace_path: None,
        metrics_path,
        commands_path,
        recovery_path,
        spill_path: report.spill_path.clone(),
        extra,
    }
}

pub fn log_smr_run_to(
    path: &Path,
    scenario: &SmrScenario,
    report: &SmrReport,
    extra: Option<serde_json::Value>,
) -> std::io::Result<()> {
    log_smr_summary_to(
        path,
        scenario,
        &SmrSummary::from_report(report, scenario),
        extra,
    )
}

/// [`log_smr_run_to`] for a flat run, which has a summary and no report.
pub fn log_smr_summary_to(
    path: &Path,
    scenario: &SmrScenario,
    summary: &SmrSummary,
    extra: Option<serde_json::Value>,
) -> std::io::Result<()> {
    append_jsonl(
        path,
        &smr_run_record(scenario, summary, None, None, None, extra),
    )
}

#[derive(Serialize)]
struct SmrAbortSummary {
    terminal: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    round: Option<usize>,
}

pub fn log_smr_abort_to(
    path: &Path,
    scenario: &SmrScenario,
    round: Option<usize>,
    extra: Option<serde_json::Value>,
) -> std::io::Result<()> {
    let (timestamp, commit, dirty) = provenance();
    let (cwd_commit, cwd_dirty) = cwd_provenance();
    append_jsonl(
        path,
        &SmrRunRecord {
            timestamp,
            commit,
            dirty,
            cwd_commit,
            cwd_dirty,
            scenario,
            amp_count: amp_count(scenario.sigma, scenario.n),
            outcome: SmrAbortSummary {
                terminal: "abort",
                round,
            },
            trace_path: None,
            metrics_path: None,
            commands_path: None,
            recovery_path: None,
            // No report to read a spill path off; the abort's file is the one without a trailer.
            spill_path: None,
            extra,
        },
    )
}

pub fn run_smr_recorded(
    scenario: &SmrScenario,
    ledger: &Path,
    extra: Option<serde_json::Value>,
) -> (SmrReport, String, String) {
    let report = run_smr(scenario);
    let (metrics_path, commands_path) = record_smr_report(scenario, &report, ledger, extra);
    (report, metrics_path, commands_path)
}

pub fn record_smr_report(
    scenario: &SmrScenario,
    report: &SmrReport,
    ledger: &Path,
    extra: Option<serde_json::Value>,
) -> (String, String) {
    let base = ledger.parent().unwrap_or(Path::new("."));
    let stem = artifact_stem("smr-", scenario.n, scenario.seed, scenario);

    let mut csv = format!("{SMR_METRICS_CSV_HEADER}\n");
    for (i, m) in report.metrics.iter().enumerate() {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            i + 1,
            m.nonbot_logs,
            m.blocked,
            m.useful,
            m.distinct_logs,
            m.max_log_len,
            m.min_executed_len,
            m.max_executed_len,
            m.arrivals,
            m.lcp_len
        ));
    }
    let metrics_file = write_new_unique(&base.join("metrics"), &stem, "csv", &csv);
    let name = metrics_file
        .file_stem()
        .expect("artifact has a stem")
        .to_string_lossy()
        .into_owned();

    let commands_file = write_new_unique(
        &base.join("commands"),
        &name,
        "json",
        &serde_json::to_string(&report.commands).expect("commands serialize"),
    );

    let recovery_path = report.recovery.as_ref().map(|rec| {
        let mut csv =
            String::from("round,noreset,reset,bot_r,window,rollbacks,max_checkpoint_window\n");
        for (i, r) in rec.rounds.iter().enumerate() {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{}\n",
                i + 1,
                r.noreset,
                r.reset,
                r.bot_r,
                r.window,
                r.rollbacks,
                r.max_checkpoint_window
            ));
        }
        let file = base.join("metrics").join(format!("{name}-recovery.csv"));
        write_new_exact(&file, &csv);
        file.to_string_lossy().into_owned()
    });

    let metrics_path = metrics_file.to_string_lossy().into_owned();
    let commands_path = commands_file.to_string_lossy().into_owned();
    let record = smr_run_record(
        scenario,
        &SmrSummary::from_report(report, scenario),
        Some(metrics_path.clone()),
        Some(commands_path.clone()),
        recovery_path,
        extra,
    );
    append_jsonl(ledger, &record).expect("run ledger write failed (set SIM_RUNLOG=off to opt out)");
    (metrics_path, commands_path)
}

pub fn run_smr_logged(scenario: &SmrScenario, extra: Option<serde_json::Value>) -> SmrReport {
    let Some(ledger) = ledger_target(None) else {
        return run_smr(scenario);
    };
    run_smr_recorded(scenario, &ledger, extra).0
}
