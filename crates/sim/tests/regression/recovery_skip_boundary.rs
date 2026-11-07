use sim::smr::{
    CommandStatus, PrefixMismatch, Proto, RepeatedCommit, SmrReport, SmrScenario, SmrState,
    SmrTerminal, run_smr, run_smr_observed,
};
use sim::sweep::{AnySpec, expand_smr, parse_specs};

/// The ratio-8 recovery spec of `experiments/e9-recovery/evidence/diag-prefix-abort-spec.json`.
const SPEC: &str = r#"[{"exp": "DBG-rec", "base": {"n": 32, "seed": 0, "k": 6, "ell": 3,
    "sigma": 1.0, "proto": {"kind": "recovery", "t_window_rounds": 26, "resend_until_acked": false},
    "injections": [], "max_rounds": 208, "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
    "schedule": {"kind": "fresh_per_round", "fraction": 0.0}},
    "ladder": [{"n": 32, "seed_base": 0, "seed_count": 1}]}]"#;

const T: usize = 26;
const HORIZON: usize = 208;

/// The seeds of `diag-prefix-abort-12seeds.txt`: every ratio-8 run that aborts on a prefix mismatch.
const ABORTING_SEEDS: [u64; 12] = [
    4330140, 4330463, 4330628, 4330680, 4330721, 4330974, 4330976, 4331153, 4331270, 4331285,
    4331438, 4331477,
];

fn scenario(seed: u64, policy: PrefixMismatch) -> SmrScenario {
    let specs = parse_specs(SPEC).expect("spec parses");
    let AnySpec::Smr(grid) = &specs[0] else {
        panic!("an SMR grid")
    };
    let (_, mut scenario) = expand_smr(grid).expect("grid expands").remove(0);
    scenario.seed = seed;
    let Proto::Recovery {
        prefix_mismatch, ..
    } = &mut scenario.proto
    else {
        panic!("a recovery grid")
    };
    *prefix_mismatch = policy;
    scenario
}

/// Per boundary where some node kept an older checkpoint: (boundary round, lagging nodes,
/// rounds until every node held the newest window again; `None` if never).
type Episode = (usize, usize, Option<usize>);

struct SkipRun {
    report: SmrReport,
    episodes: Vec<Episode>,
    final_checkpoints_agree: bool,
}

fn run_skipping(seed: u64, horizon: usize, repeated: RepeatedCommit) -> SkipRun {
    let mut episodes: Vec<Episode> = Vec::new();
    let mut final_checkpoints_agree = false;
    let mut observe = |state: &SmrState| {
        let round = state.round();
        let checkpoints: Vec<_> = (0..32)
            .map(|i| {
                state
                    .recovery_node(i)
                    .expect("recovery node")
                    .checkpoint()
                    .clone()
            })
            .collect();
        let newest = checkpoints.iter().map(|cp| cp.w).max().expect("nodes");
        let lagging = checkpoints.iter().filter(|cp| cp.w < newest).count();
        if let Some(open) = episodes.last_mut().filter(|e| e.2.is_none())
            && lagging == 0
        {
            open.2 = Some(round - open.0);
        }
        if round.is_multiple_of(T) && lagging > 0 {
            episodes.push((round, lagging, None));
        }
        final_checkpoints_agree = checkpoints
            .iter()
            .all(|cp| cp.w == newest && cp.p == checkpoints[0].p);
    };
    let mut skipping = scenario(seed, PrefixMismatch::SkipBoundary);
    skipping.max_rounds = horizon;
    skipping.repeated_commit = repeated;
    let report = run_smr_observed(&skipping, &mut observe);
    SkipRun {
        report,
        episodes,
        final_checkpoints_agree,
    }
}

#[test]
fn seed_4331438_aborts_at_round_78_under_the_default() {
    let report = run_smr(&scenario(4331438, PrefixMismatch::Abort));
    let SmrTerminal::Failed {
        attempted_round, ..
    } = report.terminal
    else {
        panic!("expected the boundary abort, got {:?}", report.terminal)
    };
    assert_eq!(attempted_round, 78);
}

#[test]
fn seed_4331438_skips_the_mismatched_boundary_and_rejoins_the_majority() {
    let run = run_skipping(4331438, HORIZON, RepeatedCommit::Execute);
    assert_eq!(run.report.terminal, SmrTerminal::Ran { rounds: HORIZON });
    assert!(
        run.report.safety_ok,
        "the committed sequences stay prefix-consistent"
    );
    let skip_at_78 = run
        .episodes
        .iter()
        .find(|e| e.0 == 78)
        .expect("the minority skips the round-78 boundary");
    assert!(
        skip_at_78.2.is_some_and(|rounds| rounds < T),
        "the minority adopts the majority checkpoint within the window: {skip_at_78:?}"
    );
    assert!(
        run.final_checkpoints_agree,
        "every node ends on one checkpoint"
    );
    assert_eq!(
        run.report.boundary_skip_events as usize, skip_at_78.1,
        "the report counts one episode per lagging node"
    );
    assert_eq!(run.report.first_boundary_skip_round, Some(78));
    assert_eq!(run.report.last_boundary_skip_round, Some(78));
    assert_eq!(run.report.max_rejoin_rounds, skip_at_78.2);
    assert_eq!(run.report.unrejoined_nodes, 0);
}

#[test]
fn a_default_run_keeps_the_skip_episode_fields_off_the_wire() {
    let report = run_smr(&scenario(4331438, PrefixMismatch::Abort));
    let json = serde_json::to_string(&report).expect("serializes");
    assert!(!json.contains("boundary_skip"), "{json}");
    assert!(!json.contains("rejoin"), "{json}");
    assert!(!json.contains("unrejoined"), "{json}");
}

/// Exploratory: `cargo test --release -p sim --test regression skip_boundary_survey -- --ignored --nocapture`.
#[test]
#[ignore]
fn skip_boundary_survey_over_the_aborting_seeds() {
    println!(
        "seed | abort-run fork_ok | repeated_commit | terminal | safety_ok | fork_ok | final agree | complete/commands | repeat skips (first round) | report equals unguarded | episodes (boundary, lagging, rounds to rejoin)"
    );
    for seed in ABORTING_SEEDS {
        let aborting = run_smr(&scenario(seed, PrefixMismatch::Abort));
        let abort_fork_ok = aborting.recovery.as_ref().map(|r| r.fork_ok);
        let mut unguarded = None;
        for repeated in [RepeatedCommit::Execute, RepeatedCommit::Skip] {
            let run = match std::panic::catch_unwind(|| run_skipping(seed, GUARD_HORIZON, repeated))
            {
                Ok(run) => run,
                Err(panic) => {
                    let message = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .unwrap_or_else(|| "non-string panic".into());
                    println!("{seed} | {abort_fork_ok:?} | {repeated:?} | PANIC: {message}");
                    continue;
                }
            };
            let fork_ok = run.report.recovery.as_ref().map(|r| r.fork_ok);
            let complete = run
                .report
                .commands
                .iter()
                .filter(|c| c.status == CommandStatus::Complete)
                .count();
            let bytes = serde_json::to_string(&run.report).expect("serializes");
            let same = unguarded.as_ref().map(|u: &String| *u == bytes);
            if repeated == RepeatedCommit::Execute {
                unguarded = Some(bytes);
            }
            println!(
                "{seed} | {abort_fork_ok:?} | {repeated:?} | {:?} | {} | {:?} | {} | {complete}/{} | {} ({:?}) | {same:?} | {:?}",
                run.report.terminal,
                run.report.safety_ok,
                fork_ok,
                run.final_checkpoints_agree,
                run.report.commands.len(),
                run.report.repeat_skips,
                run.report.first_repeat_skip_round,
                run.episodes
            );
        }
    }
}

/// Two windows past the spec horizon, so a skip at the last boundary can still rejoin.
const GUARD_HORIZON: usize = HORIZON + 2 * T;

/// The length-mismatch seeds: skipping alone re-commits a command the skipped window committed.
#[test]
fn seed_4330140_executes_a_command_twice_when_skipping_alone() {
    let run = run_skipping(4330140, GUARD_HORIZON, RepeatedCommit::Execute);
    assert_eq!(
        run.report.terminal,
        SmrTerminal::Ran {
            rounds: GUARD_HORIZON
        },
        "a double execution is recorded, not fatal"
    );
    assert!(run.report.exec_dup_round.is_some());
    let outcome = ledger_outcome(
        &{
            let mut s = scenario(4330140, PrefixMismatch::SkipBoundary);
            s.max_rounds = GUARD_HORIZON;
            s
        },
        "dup",
    );
    assert_eq!(
        outcome["exec_dup_round"],
        run.report.exec_dup_round.unwrap()
    );
}

#[test]
fn a_run_without_repeats_keeps_exec_dup_round_off_the_wire() {
    let run = run_skipping(4331438, GUARD_HORIZON, RepeatedCommit::Skip);
    assert_eq!(run.report.exec_dup_round, None);
    let json = serde_json::to_string(&run.report).expect("serializes");
    assert!(!json.contains("exec_dup_round"), "{json}");
}

#[test]
fn the_execute_once_guard_completes_both_length_mismatch_seeds_on_one_checkpoint() {
    for seed in [4330140, 4331285] {
        let run = run_skipping(seed, GUARD_HORIZON, RepeatedCommit::Skip);
        assert_eq!(
            run.report.terminal,
            SmrTerminal::Ran {
                rounds: GUARD_HORIZON
            },
            "seed {seed}"
        );
        assert!(run.report.safety_ok, "seed {seed}");
        assert!(run.final_checkpoints_agree, "seed {seed}");
        assert!(
            run.report.repeat_skips > 0,
            "seed {seed}: the skip is reported"
        );
        assert!(run.report.first_repeat_skip_round.is_some(), "seed {seed}");
    }
}

#[test]
fn a_guarded_run_without_repeats_reports_no_skips_and_keeps_its_bytes() {
    let run = run_skipping(4331438, GUARD_HORIZON, RepeatedCommit::Skip);
    assert_eq!(run.report.repeat_skips, 0);
    assert_eq!(run.report.first_repeat_skip_round, None);
    let json = serde_json::to_string(&run.report).expect("serializes");
    assert!(!json.contains("repeat_skip"), "{json}");
}

#[test]
fn the_spec_opts_in_by_name_and_keeps_the_default_off_the_wire() {
    let opted = SPEC.replace(
        r#""resend_until_acked": false}"#,
        r#""resend_until_acked": false, "prefix_mismatch": "skip_boundary"}"#,
    );
    let specs = parse_specs(&opted).expect("spec parses");
    let AnySpec::Smr(grid) = &specs[0] else {
        panic!("an SMR grid")
    };
    let (_, opted) = expand_smr(grid).expect("grid expands").remove(0);
    let wire = serde_json::to_string(&opted.proto).expect("serializes");
    assert!(
        wire.contains(r#""prefix_mismatch":"skip_boundary""#),
        "{wire}"
    );

    let default = scenario(1, PrefixMismatch::Abort);
    let wire = serde_json::to_string(&default.proto).expect("serializes");
    assert!(!wire.contains("prefix_mismatch"), "{wire}");
}

#[test]
fn the_spec_opts_into_the_guard_by_name_and_keeps_the_default_off_the_wire() {
    let opted = SPEC.replace(
        r#""sigma": 1.0,"#,
        r#""sigma": 1.0, "repeated_commit": "skip","#,
    );
    let specs = parse_specs(&opted).expect("spec parses");
    let AnySpec::Smr(grid) = &specs[0] else {
        panic!("an SMR grid")
    };
    let (_, opted) = expand_smr(grid).expect("grid expands").remove(0);
    assert_eq!(opted.repeated_commit, RepeatedCommit::Skip);
    let wire = serde_json::to_string(&opted).expect("serializes");
    assert!(wire.contains(r#""repeated_commit":"skip""#), "{wire}");

    let default = scenario(1, PrefixMismatch::Abort);
    let wire = serde_json::to_string(&default).expect("serializes");
    assert!(!wire.contains("repeated_commit"), "{wire}");
}

fn ledger_outcome(scenario: &SmrScenario, tag: &str) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("skip-episodes-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ledger = dir.join("ledger.jsonl");
    let report = sim::smr::run_smr_grid(scenario);
    sim::runlog::log_smr_run_to(&ledger, scenario, &report, None).unwrap();
    let row: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&ledger).unwrap().trim()).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    row["outcome"].clone()
}

#[test]
fn the_ledger_carries_skip_episodes_only_when_a_boundary_was_skipped() {
    let skipping = scenario(4331438, PrefixMismatch::SkipBoundary);
    let report = sim::smr::run_smr_grid(&skipping);
    let outcome = ledger_outcome(&skipping, "skipping");
    assert_eq!(outcome["boundary_skip_events"], report.boundary_skip_events);
    assert_eq!(outcome["first_boundary_skip_round"], 78);
    assert_eq!(outcome["last_boundary_skip_round"], 78);
    assert_eq!(
        outcome["max_rejoin_rounds"],
        report.max_rejoin_rounds.expect("the minority rejoined")
    );
    assert!(outcome.get("unrejoined_nodes").is_none(), "{outcome}");
    assert_eq!(
        outcome["longest_stale_rounds"],
        report
            .longest_stale_rounds
            .expect("an episode was observed")
    );

    let clean = ledger_outcome(&scenario(4330000, PrefixMismatch::SkipBoundary), "clean");
    for key in [
        "boundary_skip_events",
        "first_boundary_skip_round",
        "last_boundary_skip_round",
        "max_rejoin_rounds",
        "longest_stale_rounds",
        "unrejoined_nodes",
        "horizon_skip_nodes",
    ] {
        assert!(clean.get(key).is_none(), "{key}: {clean}");
    }
}

/// Ratio-4 seed where both repairs are on and a minority P that prefixes its holder's log commits:
/// the run forks, but no server's executed sequence ever holds a (client, sn) twice.
#[test]
fn seed_4420464_forks_without_a_double_execution_and_runs_to_the_horizon() {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(serde_json::json!({
        "n": 32, "seed": 4420464, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": 13, "resend_until_acked": false, "prefix_mismatch": "skip_boundary"},
        "repeated_commit": "skip", "injections": [], "max_rounds": 130,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
    }))
    .expect("spec parses");
    let report = sim::smr::run_smr_grid(&SmrScenario::try_from(spec).expect("valid"));
    assert_eq!(report.terminal, SmrTerminal::Ran { rounds: 130 });
    assert_eq!(report.exec_dup_round, None);
    assert!(!report.safety_ok);
}
