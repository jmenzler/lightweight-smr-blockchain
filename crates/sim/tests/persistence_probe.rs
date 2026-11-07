//! RQ2 persistence probe: displacement metric, per-round split-brain oracle,
//! and replay equivalence with the batch runner. Design registered in
//! experiments/rq2-commitment-errors/prereg-persistence.md.

use protocol::compact::{ClientCommand, Entry};
use protocol::shared_state::ExecSeq;
use sim::persistence::{
    PROBE_CSV_HEADER, ProbeOptions, detail_path, displacement, duplicate_executions, forked_branch,
    max_duplicate_executions, max_sn_regressions, mutual_fork_pair, observe, parse_probe_specs,
    partition_branches, probe_row, probe_run, sn_regressions, transience,
};
use sim::smr::{SmrScenario, prefixes_consistent, run_smr};
use sim::spec::SmrScenarioSpec;

/// Executed-sequence sugar: `c(op)` is a command, `nop(x)` a filler.
fn c(op: u64) -> Entry {
    Entry::Cmd(ClientCommand {
        client: 1000 + op as u32,
        sn: 1,
        op,
    })
}

fn seq(ops: &[u64]) -> Vec<Entry> {
    ops.iter().copied().map(c).collect()
}

#[test]
fn identical_branches_displace_nothing() {
    let d = displacement(&seq(&[1, 2, 3]), &seq(&[1, 2, 3]));
    assert_eq!((d.common, d.lcs, d.d), (3, 3, 0));
    assert!(d.displaced_idx.is_empty());
}

/// The registered shift-invariance property: one command jumping past a whole
/// suffix costs D = 1, not "the entire suffix" (prereg § E1).
#[test]
fn one_command_displacing_a_whole_suffix_costs_one() {
    // A executes x first, B executes it last; a,b,c shift by one position.
    let a = seq(&[9, 1, 2, 3]);
    let b = seq(&[1, 2, 3, 9]);
    let d = displacement(&a, &b);
    assert_eq!((d.common, d.lcs, d.d), (4, 3, 1));
    assert_eq!(d.displaced_idx, vec![0], "x sits at A-index 0");
}

#[test]
fn a_transposed_pair_displaces_one_command() {
    let d = displacement(&seq(&[1, 2, 3, 4]), &seq(&[1, 3, 2, 4]));
    assert_eq!((d.common, d.lcs, d.d), (4, 3, 1));
}

#[test]
fn a_reversed_branch_displaces_all_but_one() {
    let d = displacement(&seq(&[1, 2, 3]), &seq(&[3, 2, 1]));
    assert_eq!((d.common, d.lcs, d.d), (3, 1, 2));
}

#[test]
fn commands_missing_from_one_branch_leave_the_common_order_alone() {
    // B has executed 7 and 8 that A has not seen yet: not common, so they
    // neither count toward m nor displace anything.
    let d = displacement(&seq(&[1, 2, 3]), &seq(&[1, 7, 2, 8, 3]));
    assert_eq!((d.common, d.lcs, d.d), (3, 3, 0));
}

#[test]
fn nops_are_not_commands_and_never_displace() {
    let a = vec![Entry::Nop(0), c(1), Entry::Nop(7), c(2)];
    let b = vec![c(1), Entry::Nop(0), c(2), Entry::Nop(9)];
    let d = displacement(&a, &b);
    assert_eq!((d.common, d.lcs, d.d), (2, 2, 0));
}

/// Sub-threshold servers re-execute commands: `execute_aged_prefix` pushes
/// every aged log entry unconditionally, and the merged log is never filtered
/// against what the server already executed. D must stay a pure ORDER metric,
/// so it reads each command once — re-execution is reported by `dup_exec`.
#[test]
fn a_re_executed_command_does_not_displace_itself() {
    let d = displacement(&[c(1), c(2), c(1), c(3)], &seq(&[1, 2, 3]));
    assert_eq!((d.common, d.lcs, d.d), (3, 3, 0));

    let both = displacement(&[c(1), c(2), c(1)], &[c(2), c(1), c(2)]);
    assert_eq!(
        (both.common, both.lcs, both.d),
        (2, 1, 1),
        "first executions order as 1,2 against 2,1 — one displacement"
    );
}

#[test]
fn branches_sharing_no_command_have_no_common_order_to_displace() {
    let d = displacement(&seq(&[1, 2]), &seq(&[3, 4]));
    assert_eq!((d.common, d.lcs, d.d), (0, 0, 0));
}

/// The probe checks consistency over borrowed chunked sequences (cloning every
/// sequence every round is what makes `executed_seqs` unusable as an oracle
/// feed), so every input form must give the slice-comparison verdict, case for case.
#[test]
fn every_oracle_input_form_matches_the_slice_reference() {
    fn reference(seqs: &[Vec<Entry>]) -> bool {
        let Some(longest) = seqs.iter().max_by_key(|e| e.len()) else {
            return true;
        };
        seqs.iter().all(|e| longest[..e.len()] == e[..])
    }
    let cases: Vec<Vec<Vec<Entry>>> = vec![
        vec![],
        vec![seq(&[])],
        vec![seq(&[1, 2, 3]), seq(&[1, 2, 3])],
        vec![seq(&[1, 2]), seq(&[1, 2, 3])],
        vec![seq(&[1, 2, 9]), seq(&[1, 2, 3])],
        vec![seq(&[1, 2, 3]), seq(&[1, 2]), seq(&[1, 2, 3, 4])],
        vec![seq(&[1, 2, 3]), seq(&[]), seq(&[9])],
        vec![vec![Entry::Nop(0), c(1)], vec![Entry::Nop(0), c(2)]],
    ];
    for case in &cases {
        let want = reference(case);
        let borrowed: Vec<&[Entry]> = case.iter().map(Vec::as_slice).collect();
        let chunked: Vec<ExecSeq> = case.iter().map(|s| s.iter().cloned().collect()).collect();
        let chunked_refs: Vec<&ExecSeq> = chunked.iter().collect();
        assert_eq!(prefixes_consistent(case), want, "owned: {case:?}");
        assert_eq!(prefixes_consistent(&borrowed), want, "borrowed: {case:?}");
        assert_eq!(
            prefixes_consistent(&chunked_refs),
            want,
            "chunked: {case:?}"
        );
    }
}

#[test]
fn a_consistent_population_is_one_branch() {
    let a = seq(&[1, 2, 3]);
    let b = seq(&[1, 2, 3]);
    let seqs: Vec<&[Entry]> = vec![&a, &b];
    assert_eq!(partition_branches(&seqs), vec![vec![0, 1]]);
}

/// Branches come back largest first so the estimand's "two largest classes"
/// is read off the front; equal sizes order by lowest member id, so a rerun
/// picks the same representatives.
#[test]
fn branches_are_ordered_by_size_then_by_lowest_member() {
    let x = seq(&[1, 2]);
    let y = seq(&[1, 9]);
    let z = seq(&[1, 9]);
    let seqs: Vec<&[Entry]> = vec![&x, &y, &z];
    assert_eq!(partition_branches(&seqs), vec![vec![1, 2], vec![0]]);

    let seqs: Vec<&[Entry]> = vec![&y, &x, &z];
    assert_eq!(
        partition_branches(&seqs),
        vec![vec![0, 2], vec![1]],
        "the two-member branch still leads"
    );

    let p = seq(&[1]);
    let q = seq(&[2]);
    let seqs: Vec<&[Entry]> = vec![&q, &p];
    assert_eq!(
        partition_branches(&seqs),
        vec![vec![0], vec![1]],
        "equal sizes keep population order"
    );
}

/// The registered estimand reads D between the two LARGEST branches, and those
/// two can sit on the same side of the fork — a shorter branch is a distinct
/// sequence but still prefix-consistent. The instrument therefore also names
/// the largest branch that actually disagrees, so an analysis can see whether
/// the registered pair straddled the split.
#[test]
fn the_forked_branch_is_the_largest_one_that_actually_disagrees() {
    let long = seq(&[1, 2, 3]);
    let short = seq(&[1, 2]);
    let other = seq(&[1, 3, 2]);
    let seqs: Vec<&[Entry]> = vec![&long, &long, &long, &short, &short, &other];

    let branches = partition_branches(&seqs);
    assert_eq!(branches[0], vec![0, 1, 2]);
    assert_eq!(
        branches[1],
        vec![3, 4],
        "the runner-up agrees with the lead"
    );
    assert_eq!(displacement(seqs[0], seqs[3]).d, 0);

    assert_eq!(forked_branch(&seqs), Some(5));
    assert_eq!(displacement(seqs[0], seqs[5]).d, 1);
}

#[test]
fn a_population_that_agrees_has_no_forked_branch() {
    let long = seq(&[1, 2, 3]);
    let short = seq(&[1, 2]);
    let seqs: Vec<&[Entry]> = vec![&long, &short];
    assert_eq!(forked_branch(&seqs), None);
}

/// Red flag 1 of the prereg: a command executed twice on one server. The
/// compact rule has no gate against it — `execute_aged_prefix` pushes every
/// aged log entry — so it has to be measured rather than assumed away.
#[test]
fn a_command_executed_twice_is_a_duplicate_execution() {
    let once = seq(&[1, 2, 3]);
    assert_eq!(duplicate_executions(&once), 0);

    let twice = vec![c(1), c(2), c(1)];
    assert_eq!(duplicate_executions(&twice), 1);

    // Identity is (client, sn) — the diagnosis the pool-model fidelity note
    // demands — not the op, so a re-executed command counts even under a
    // different op.
    let regressed = vec![
        Entry::Cmd(ClientCommand {
            client: 5,
            sn: 2,
            op: 100,
        }),
        Entry::Cmd(ClientCommand {
            client: 5,
            sn: 2,
            op: 200,
        }),
    ];
    assert_eq!(duplicate_executions(&regressed), 1);
}

/// Re-execution and sn regression are DIFFERENT mechanisms and need different
/// columns. Re-execution is gate-free replay of the same command; an sn
/// regression is two distinct ops accepted at one `(client, sn)`, which is the
/// only thing that can re-open §4's acceptance gate. Counting is per
/// `(client, sn)` pair, so one pair carrying three ops is one regression.
#[test]
fn an_sn_regression_needs_two_different_ops_at_one_client_sn() {
    let cmd = |client: u32, sn: u64, op: u64| Entry::Cmd(ClientCommand { client, sn, op });

    assert_eq!(sn_regressions(&seq(&[1, 2, 3])), 0);
    assert_eq!(
        sn_regressions(&[c(1), c(2), c(1)]),
        0,
        "the same command replayed is re-execution, not a regression"
    );
    assert_eq!(sn_regressions(&[cmd(5, 2, 100), cmd(5, 2, 200)]), 1);
    assert_eq!(
        sn_regressions(&[cmd(5, 2, 100), cmd(5, 2, 200), cmd(5, 2, 300)]),
        1,
        "one pair, however many ops it collected"
    );
    assert_eq!(
        sn_regressions(&[cmd(5, 2, 100), cmd(5, 2, 200), cmd(7, 1, 10), cmd(7, 1, 11)]),
        2
    );
}

#[test]
fn the_population_sn_regression_count_is_the_worst_servers() {
    let cmd = |client: u32, sn: u64, op: u64| Entry::Cmd(ClientCommand { client, sn, op });
    let two = [cmd(5, 2, 100), cmd(5, 2, 200), cmd(7, 1, 10), cmd(7, 1, 11)];
    let one = [cmd(5, 2, 100), cmd(5, 2, 200)];
    let clean = seq(&[1, 2]);
    let seqs: Vec<&[Entry]> = vec![&two, &one, &clean];
    assert_eq!(max_sn_regressions(&seqs), 2);
}

/// The red flag reads "a command executed twice on ONE server", so the
/// population figure is the worst server's count — summing across branches
/// would report a number no server ever had.
#[test]
fn the_population_duplicate_count_is_the_worst_servers_not_the_sum() {
    let twice = vec![c(1), c(2), c(1), c(2)];
    let once = vec![c(1), c(2), c(1)];
    let clean = seq(&[1, 2]);
    let seqs: Vec<&[Entry]> = vec![&twice, &twice, &once, &clean];
    assert_eq!(max_duplicate_executions(&seqs), 2);
}

#[test]
fn nops_repeat_freely_and_are_not_duplicate_executions() {
    assert_eq!(
        duplicate_executions(&[Entry::Nop(0), c(1), Entry::Nop(0)]),
        0
    );
}

/// Arm P (β = 0) has no ⊥ events, so nothing can rewrite an executed prefix
/// and the divergence is permanent by construction — H is undefined and φ = 1.
#[test]
fn a_split_that_never_closes_has_no_healing_round() {
    let t = transience(10, &[true; 5]);
    assert_eq!((t.r_h, t.heal_rounds), (None, None));
    assert_eq!((t.episodes, t.inconsistent_rounds), (1, 5));
    assert_eq!(t.phi, 1.0);
}

#[test]
fn healing_time_counts_from_the_violation_round_to_the_first_clean_round() {
    let t = transience(10, &[true, false, false]);
    assert_eq!((t.r_h, t.heal_rounds), (Some(11), Some(1)));
    assert_eq!((t.episodes, t.inconsistent_rounds), (1, 1));
    assert_eq!(t.phi, 1.0 / 3.0);
}

/// φ is the headline whenever the split recurs: a median H over episodes would
/// read "transient" while the system is split most of the time.
#[test]
fn a_recurring_split_counts_every_stretch_as_its_own_episode() {
    let t = transience(10, &[true, false, true, true, false]);
    assert_eq!((t.r_h, t.heal_rounds), (Some(11), Some(1)));
    assert_eq!((t.episodes, t.inconsistent_rounds), (2, 3));
    assert_eq!(t.phi, 3.0 / 5.0);
}

#[test]
fn a_single_split_round_at_the_horizon_is_one_unhealed_episode() {
    let t = transience(10, &[true]);
    assert_eq!((t.r_h, t.episodes, t.inconsistent_rounds), (None, 1, 1));
    assert_eq!(t.phi, 1.0);
}

fn scenario(json: &str) -> SmrScenario {
    let spec: SmrScenarioSpec = serde_json::from_str(json).expect("spec parses");
    SmrScenario::try_from(spec).expect("valid scenario")
}

/// A compact traffic cell at `t_commit`, deep below the safety boundary when
/// `t_commit` is small — the regime the persistence dispatch lives in.
fn cell(t_commit: u64, beta: f64, seed: u64, rounds: usize) -> SmrScenario {
    scenario(&format!(
        r#"{{"n": 32, "seed": {seed}, "k": 6, "ell": 3, "sigma": 1.0,
            "proto": {{"kind": "compact", "t_commit_rounds": {t_commit}}},
            "injections": [], "max_rounds": {rounds},
            "schedule": {{"kind": "fresh_per_round", "fraction": {beta}}},
            "traffic": {{"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]}}}}"#
    ))
}

/// The probe observes; it must not perturb. Stepping by hand has to draw from
/// the seeded stream in exactly the batch runner's order, so the report it
/// ends on is the report `run_smr` would have produced — otherwise every
/// replayed boundary-map seed is a different run.
#[test]
fn the_probe_reproduces_the_batch_runner_report_without_blocking() {
    let scenario = cell(40, 0.0, 4242, 60);
    let probe = probe_run(&scenario, &ProbeOptions::default()).expect("fresh compact cell");
    assert_eq!(probe.report, run_smr(&scenario));
}

/// The blocked-mask draw is the one stage a hand-stepped session can get out
/// of order, so it gets its own pin at beta > 0.
#[test]
fn the_probe_reproduces_the_batch_runner_report_under_blocking() {
    let scenario = cell(40, 0.1, 4242, 60);
    let probe = probe_run(&scenario, &ProbeOptions::default()).expect("fresh compact cell");
    assert_eq!(probe.report, run_smr(&scenario));
}

#[test]
fn the_probe_reproduces_the_batch_runner_report_on_a_run_that_splits_its_brain() {
    let scenario = cell(6, 0.1, 4242, 80);
    let probe = probe_run(&scenario, &ProbeOptions::default()).expect("fresh compact cell");
    assert!(!probe.report.safety_ok, "sub-threshold T must split");
    assert_eq!(probe.report, run_smr(&scenario));
}

/// Prereg OBLIGATION 2: the probe's per-round oracle must agree with the
/// engine's own latch, which flips in the round the split brain appears.
#[test]
fn the_probe_oracle_flips_in_the_same_round_as_the_engine_latch() {
    let probe = probe_run(&cell(6, 0.0, 4242, 80), &ProbeOptions::default()).expect("cell");
    assert!(probe.r_v.is_some(), "sub-threshold T must split");
    assert_eq!(probe.r_v, probe.latch_round);
}

#[test]
fn a_run_that_never_splits_has_no_violation_round_and_no_observations() {
    let probe = probe_run(&cell(40, 0.0, 4242, 60), &ProbeOptions::default()).expect("cell");
    assert_eq!(probe.r_v, None);
    assert_eq!(probe.latch_round, None);
    assert!(probe.transience.is_none());
    assert!(probe.split_after_rv.is_empty());
    assert!(probe.observations.is_empty());
}

/// `compile_blocking` draws the mask set up front for every schedule except
/// fresh-per-round, and those draws are `pub(crate)` — a bin cannot reproduce
/// the stream, so replaying one would silently be a different run.
#[test]
fn a_schedule_the_probe_cannot_replay_is_rejected_loudly() {
    let permanent = scenario(
        r#"{"n": 32, "seed": 1, "k": 6, "ell": 3, "sigma": 1.0,
            "proto": {"kind": "compact", "t_commit_rounds": 20},
            "injections": [], "max_rounds": 20,
            "schedule": {"kind": "permanent", "fraction": 0.1},
            "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]}}"#,
    );
    let err = probe_run(&permanent, &ProbeOptions::default()).expect_err("must reject");
    assert!(err.contains("fresh_per_round"), "unhelpful message: {err}");
}

#[test]
fn manual_blocks_are_rejected_because_they_union_after_the_mask_draw() {
    let manual = scenario(
        r#"{"n": 32, "seed": 1, "k": 6, "ell": 3, "sigma": 1.0,
            "proto": {"kind": "compact", "t_commit_rounds": 20},
            "injections": [], "max_rounds": 20,
            "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
            "manual_blocks": [{"node": 0, "from_round": 2}],
            "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]}}"#,
    );
    let err = probe_run(&manual, &ProbeOptions::default()).expect_err("must reject");
    assert!(err.contains("manual_blocks"), "unhelpful message: {err}");
}

/// The extended engine keeps no executed sequences, so there is nothing for
/// the oracle to read — a silently empty readout would report "consistent".
#[test]
fn a_non_compact_proto_is_rejected() {
    let extended = scenario(
        r#"{"n": 32, "seed": 1, "k": 6, "ell": 3, "sigma": 1.0,
            "proto": {"kind": "extended"},
            "injections": [{"round": 1, "client": 1, "op": 7}], "max_rounds": 20,
            "schedule": {"kind": "fresh_per_round", "fraction": 0.0}}"#,
    );
    let err = probe_run(&extended, &ProbeOptions::default()).expect_err("must reject");
    assert!(err.contains("compact"), "unhelpful message: {err}");
}

/// Registered cost remedy: stop each probe run `stop_rounds_after_violation`
/// rounds past r_v. The trajectory prefix is unaffected — the horizon only
/// ever truncates a deterministic stream.
#[test]
fn the_post_violation_cap_stops_the_run_and_says_so() {
    let scenario = cell(6, 0.1, 4242, 100);
    let opts = ProbeOptions {
        stop_rounds_after_violation: Some(10),
        ..ProbeOptions::default()
    };
    let capped = probe_run(&scenario, &opts).expect("cell");
    let r_v = capped.r_v.expect("sub-threshold T must split");
    assert!(capped.truncated);
    assert_eq!(capped.rounds, r_v + 10);

    let full = probe_run(&scenario, &ProbeOptions::default()).expect("cell");
    assert!(!full.truncated);
    assert_eq!(full.rounds, 100);
    assert_eq!(full.r_v, capped.r_v, "the prefix is the same run");
}

/// Prereg OBLIGATION 2, on a committed ledgered run: `.runs/sim-runs.jsonl`
/// records this rq2-tn-boundary cell/seed as safety_ok = false. The grid ran
/// it LEAN and the lean-vs-full pins deliberately cover only non-violating
/// runs, so the probe reproducing the committed verdict is a real check.
#[test]
fn a_committed_violating_boundary_seed_replays_to_its_ledgered_verdict() {
    let boundary = scenario(
        r#"{"n": 64, "seed": 970001, "k": 6, "ell": 3, "sigma": 1.0,
            "proto": {"kind": "compact", "t_commit_rounds": 20},
            "injections": [], "max_rounds": 2500,
            "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
            "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]}}"#,
    );
    let opts = ProbeOptions {
        // The ledgered run went the full 2500 rounds; the committed bit under
        // test is its verdict, which is settled by round 21. Out-of-suite
        // release validation runs the whole horizon.
        stop_rounds_after_violation: Some(30),
        expected_safety: Some(false),
    };
    let probe = probe_run(&boundary, &opts).expect("boundary cell");
    assert_eq!(probe.replay_ok, Some(true));
    assert!(!probe.report.safety_ok);
    assert_eq!(
        probe.r_v,
        Some(21),
        "first violation lands one round past T"
    );
    assert_eq!(probe.latch_round, probe.r_v);
}

/// Deriving the detail file by swapping `.csv` for `.json` overwrites the input
/// spec whenever the two share a stem (`x.json` in, `x.csv` out), so the detail
/// name carries its own marker and can never land on a spec file.
#[test]
fn the_detail_file_never_lands_on_a_spec_file() {
    assert_eq!(
        detail_path("evidence/p-n64-b0.csv"),
        "evidence/p-n64-b0-runs.json"
    );
    assert_eq!(detail_path("ledger-check"), "ledger-check-runs.json");
    assert_ne!(detail_path("cells.csv"), "cells.json");
}

const CELL_SPEC: &str = r#"{
    "exp": "rq2-persistence",
    "seeds": [970001, 970002, 970005],
    "expect_violating": [970001, 970005],
    "stop_rounds_after_violation": 500,
    "base": {
        "n": 64, "seed": 0, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "compact", "t_commit_rounds": 29},
        "injections": [], "max_rounds": 2500,
        "schedule": {"kind": "fresh_per_round", "fraction": 0.0},
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]}
    }
}"#;

/// The spec's seed list becomes one run per seed, in spec order, with the
/// cell's parameters carried over unchanged — the probe budget cap and the
/// ascending-seed selection rule are applied when the list is WRITTEN, so the
/// runner must not reorder or filter it.
#[test]
fn a_cell_spec_expands_into_one_run_per_seed_in_order() {
    let specs = parse_probe_specs(CELL_SPEC).expect("spec parses");
    assert_eq!(specs.len(), 1);
    let runs = specs[0].expand().expect("valid cell");
    assert_eq!(
        runs.iter().map(|(s, _)| s.seed).collect::<Vec<_>>(),
        vec![970001, 970002, 970005]
    );
    assert_eq!(runs[0].0.n, 64);
    assert_eq!(runs[0].0.max_rounds, 2500);
}

/// Replay validation is per seed: only the seeds whose committed ledger row
/// says `safety_ok = false` carry an expectation, so a screened seed with no
/// ledger history is not silently asserted against.
#[test]
fn only_the_seeds_with_a_committed_verdict_carry_a_replay_expectation() {
    let specs = parse_probe_specs(CELL_SPEC).expect("spec parses");
    let runs = specs[0].expand().expect("valid cell");
    let expectations: Vec<Option<bool>> = runs.iter().map(|(_, o)| o.expected_safety).collect();
    assert_eq!(expectations, vec![Some(false), None, Some(false)]);
    assert!(
        runs.iter()
            .all(|(_, o)| o.stop_rounds_after_violation == Some(500))
    );
}

#[test]
fn a_spec_file_may_hold_an_array_of_cells() {
    let array = format!("[{CELL_SPEC},{CELL_SPEC}]");
    assert_eq!(parse_probe_specs(&array).expect("array parses").len(), 2);
}

#[test]
fn an_unknown_spec_field_is_rejected_rather_than_ignored() {
    let typo = CELL_SPEC.replace("\"seeds\"", "\"seed_list\"");
    assert!(parse_probe_specs(&typo).is_err());
}

fn column<'a>(row: &'a str, name: &str) -> &'a str {
    let i = PROBE_CSV_HEADER
        .split(',')
        .position(|h| h == name)
        .unwrap_or_else(|| panic!("no column {name} in {PROBE_CSV_HEADER}"));
    row.split(',').nth(i).expect("row is as wide as the header")
}

#[test]
fn every_row_is_exactly_as_wide_as_the_header() {
    let width = PROBE_CSV_HEADER.split(',').count();
    for scenario in [cell(40, 0.0, 4242, 60), cell(6, 0.1, 4242, 80)] {
        let probe = probe_run(&scenario, &ProbeOptions::default()).expect("cell");
        let row = probe_row("rq2-persistence", &scenario, &probe);
        assert_eq!(row.split(',').count(), width, "row: {row}");
    }
}

/// A run that never split leaves every persistence column empty rather than
/// reporting a zero — a zero displacement and "no error happened" are not the
/// same reading, and the estimands are conditional on an error.
#[test]
fn a_clean_run_leaves_the_persistence_columns_empty() {
    let scenario = cell(40, 0.0, 4242, 60);
    let probe = probe_run(&scenario, &ProbeOptions::default()).expect("cell");
    let row = probe_row("rq2-persistence", &scenario, &probe);
    assert_eq!(column(&row, "safety"), "true");
    for empty in [
        "r_v",
        "latch_round",
        "r_h",
        "phi",
        "d_rv",
        "delta_d",
        "d_fork_1t",
        "delta_d_fork",
    ] {
        assert_eq!(column(&row, empty), "", "column {empty} in {row}");
    }
}

#[test]
fn a_split_run_reports_its_cell_and_its_violation_round() {
    let scenario = cell(6, 0.1, 4242, 80);
    let probe = probe_run(&scenario, &ProbeOptions::default()).expect("cell");
    let row = probe_row("rq2-persistence", &scenario, &probe);
    assert_eq!(column(&row, "exp"), "rq2-persistence");
    assert_eq!(column(&row, "n"), "32");
    assert_eq!(column(&row, "beta"), "0.1");
    assert_eq!(column(&row, "rate"), "4");
    assert_eq!(column(&row, "t_commit"), "6");
    assert_eq!(column(&row, "horizon"), "80");
    assert_eq!(column(&row, "seed"), "4242");
    assert_eq!(column(&row, "client_model"), "unique");
    assert_eq!(column(&row, "safety"), "false");
    assert_eq!(
        column(&row, "r_v"),
        probe.r_v.expect("split").to_string(),
        "row: {row}"
    );
    assert_eq!(column(&row, "latch_agrees"), "true");
}

/// Every split round has a branch on the other side of the fork, so the fork
/// columns are populated whenever the registered D columns are.
#[test]
fn a_split_run_also_reports_the_displacement_across_the_actual_fork() {
    let scenario = cell(6, 0.1, 4242, 80);
    let probe = probe_run(&scenario, &ProbeOptions::default()).expect("cell");
    let row = probe_row("rq2-persistence", &scenario, &probe);
    for populated in ["fork_branch_rv", "d_fork_rv", "d_fork_last"] {
        assert_ne!(column(&row, populated), "", "column {populated} in {row}");
    }
    let first = probe.observations.first().expect("split run observes");
    assert!(first.fork_branch.is_some_and(|f| f >= 1));
}

/// The registered blast-radius pair is A against the largest branch that is NOT
/// prefix-consistent with A, and the registered ΔD baseline is D(r_v + T). The
/// fork reading therefore needs its own r_v + T column: without it the
/// registered growth quantity is not computable from the CSV at all.
#[test]
fn the_fork_displacement_is_reported_at_the_registered_delta_d_baseline() {
    let scenario = cell(6, 0.1, 4242, 80);
    let probe = probe_run(&scenario, &ProbeOptions::default()).expect("cell");
    let row = probe_row("rq2-persistence", &scenario, &probe);
    let r_v = probe.r_v.expect("sub-threshold T must split");

    let at_1t = probe
        .observations
        .iter()
        .find(|o| o.round == r_v + 6)
        .expect("the r_v + T observation");
    let last = probe.observations.last().expect("split run observes");

    let base = at_1t.d_fork.expect("this cell forks against A");
    let end = last.d_fork.expect("this cell forks against A");
    assert_eq!(column(&row, "d_fork_1t"), base.to_string());
    assert_eq!(
        column(&row, "delta_d_fork"),
        (end as i64 - base as i64).to_string(),
        "row: {row}"
    );
}

/// Re-execution and sn regression get their own columns: below the threshold
/// the first is routine, and only the second bears on whether a violation can
/// still be read as a pure log-position disagreement.
#[test]
fn re_execution_and_sn_regression_are_reported_separately() {
    let scenario = cell(6, 0.1, 4242, 80);
    let probe = probe_run(&scenario, &ProbeOptions::default()).expect("cell");
    let row = probe_row("rq2-persistence", &scenario, &probe);

    let dup: usize = column(&row, "dup_exec_max").parse().expect("a count");
    let regress: usize = column(&row, "sn_regress_max").parse().expect("a count");
    assert!(dup > 0, "a sub-threshold cell re-executes commands: {row}");
    assert_eq!(
        regress, 0,
        "the unique client model mints one op per (client, sn): {row}"
    );

    let first = probe.observations.first().expect("split run observes");
    assert_eq!(first.sn_regress, 0);
}

/// A seed the ledger recorded as violating that does not violate under the
/// probe (or the reverse) is red flag 3 — it must surface as data, not as a
/// panic, so a screening pass can report it per seed.
#[test]
fn a_replay_that_contradicts_the_committed_verdict_is_reported_not_panicked() {
    let opts = ProbeOptions {
        expected_safety: Some(false),
        ..ProbeOptions::default()
    };
    let probe = probe_run(&cell(40, 0.0, 4242, 60), &opts).expect("cell");
    assert!(probe.report.safety_ok, "this cell is above the boundary");
    assert_eq!(probe.replay_ok, Some(false));
}

/// `None` from `forked_branch` is a LEGITIMATE outcome on a split round, not an
/// impossible one: the split test compares against the LONGEST sequence while
/// `forked_branch` searches against the LARGEST branch, and the plurality branch can be
/// a common prefix of both divergent sides. The estimand path must report that as an
/// absent reading, per the probe's empty-not-zero contract, never panic.
#[test]
fn a_fork_the_plurality_branch_sits_above_reads_empty_rather_than_panicking() {
    let short = seq(&[1, 2]);
    let b = seq(&[1, 2, 3, 4]);
    let c = seq(&[1, 2, 4, 3]);
    let seqs: Vec<&[Entry]> = vec![&short, &short, &short, &b, &b, &c];

    assert!(
        !prefixes_consistent(&seqs),
        "b and c fork: the round IS split"
    );
    assert_eq!(
        partition_branches(&seqs)[0].len(),
        3,
        "the SHORT branch is the plurality"
    );
    assert_eq!(
        forked_branch(&seqs),
        None,
        "nothing contradicts the plurality branch"
    );

    let o = observe(7, &seqs, 0);
    assert_eq!(o.fork_branch, None, "no fork against A: absent, not zero");
    assert_eq!(o.d_fork, None);
    assert_eq!(o.branches, 3);
}

/// The generalised reading — D across the two largest MUTUALLY prefix-inconsistent
/// branches — stays measurable exactly where the registered pair is undefined. Reported
/// as a diagnostic only; it feeds no registered gate.
#[test]
fn the_mutual_fork_reading_survives_where_the_registered_pair_is_undefined() {
    let short = seq(&[1, 2]);
    let b = seq(&[1, 2, 3, 4]);
    let c = seq(&[1, 2, 4, 3]);
    let seqs: Vec<&[Entry]> = vec![&short, &short, &short, &b, &b, &c];

    assert_eq!(
        mutual_fork_pair(&seqs),
        Some((3, 5)),
        "the b/c fork, largest first"
    );
    let o = observe(7, &seqs, 0);
    assert_eq!(o.d_mutual, Some(displacement(&b, &c).d));
    assert_eq!(
        o.d_mutual,
        Some(1),
        "one transposition across the real fork"
    );
}

/// Value preservation: where the plurality branch IS on one side of the fork — every
/// case the instrument has measured to date — the registered reading is unchanged.
#[test]
fn a_plurality_on_one_side_of_the_fork_reads_exactly_as_before() {
    let a = seq(&[1, 2, 3]);
    let d = seq(&[1, 3, 2]);
    let seqs: Vec<&[Entry]> = vec![&a, &a, &a, &d];

    assert_eq!(
        forked_branch(&seqs),
        Some(3),
        "the deviant branch contradicts A"
    );
    let o = observe(9, &seqs, 0);
    assert_eq!(o.fork_branch, Some(1));
    assert_eq!(o.d_fork, Some(1), "one displaced command across the fork");
    assert_eq!(
        o.d_mutual, o.d_fork,
        "generalised == registered when A is on a side"
    );
}

fn probe_cell(n: usize, t: u64, beta: f64, pmf: &str, seed: u64, rounds: usize) -> SmrScenario {
    scenario(&format!(
        r#"{{"n": {n}, "seed": {seed}, "k": 6, "ell": 3, "sigma": 1.0,
            "proto": {{"kind": "compact", "t_commit_rounds": {t}}},
            "injections": [], "max_rounds": {rounds},
            "schedule": {{"kind": "fresh_per_round", "fraction": {beta}}},
            "traffic": {{"kind": "pmf", "arrivals_pmf": {pmf}}}}}"#
    ))
}

/// Independent replay: rounds in which some server's executed sequence did not extend its previous one.
fn retraction_rounds(scenario: &SmrScenario, upto_round: usize) -> Vec<usize> {
    let sim::BlockSchedule::FreshPerRound { fraction } = scenario.schedule else {
        panic!("fresh schedule expected");
    };
    let mut state = sim::smr::SmrState::new_with_certs(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &scenario.injections,
        scenario.traffic.as_deref().unwrap_or(&[]),
        scenario.client_model,
        scenario.partition.as_deref(),
        false,
        scenario.merge_policy,
        scenario.repeated_commit,
    );
    let blocked = state.blocked_count(fraction);
    let mut prev: Vec<Vec<Entry>> = vec![Vec::new(); scenario.n];
    let mut rounds = Vec::new();
    for round in 1..=upto_round {
        state.step_sampled(blocked);
        for (i, p) in prev.iter_mut().enumerate() {
            let cur = state.executed_entries(i).expect("compact").to_vec();
            if (cur.len() < p.len() || cur[..p.len()] != p[..]) && rounds.last() != Some(&round) {
                rounds.push(round);
            }
            *p = cur;
        }
    }
    rounds
}

// Def 1.5, p. 8: a retraction in a run that never splits still shows in the CSV.
#[test]
fn a_retraction_without_a_split_appears_in_the_probe_row() {
    for (n, t, beta, seed) in [
        (24usize, 3u64, 0.25f64, 3u64),
        (24, 3, 0.25, 21),
        (32, 4, 0.2, 36),
    ] {
        let scenario = probe_cell(n, t, beta, "[0,0.5,0.5]", seed, 150);
        let probe = probe_run(&scenario, &ProbeOptions::default()).expect("probe");
        let rounds = retraction_rounds(&scenario, probe.rounds);
        assert!(
            probe.r_v.is_none() && !rounds.is_empty(),
            "precondition, seed {seed}"
        );
        let row = probe_row("rq2", &scenario, &probe);
        assert_eq!(
            column(&row, "shrink_events"),
            "",
            "unchanged: defined for split runs only"
        );
        assert!(column(&row, "retractions").parse::<u64>().expect("filled") >= 1);
        assert_eq!(
            column(&row, "first_retraction_round"),
            rounds[0].to_string()
        );
        assert_eq!(column(&row, "heals"), "", "no split, so nothing to heal");
    }
}

// p. 8 + p. 29: a return to consistency after a split is reached by rewriting executed commands.
#[test]
fn every_heal_is_labelled_with_the_retractions_it_took() {
    for seed in [970012u64, 970007] {
        let scenario = probe_cell(64, 16, 0.1, "[0,0,0,0,1]", seed, 200);
        let opts = ProbeOptions {
            stop_rounds_after_violation: Some(48),
            ..ProbeOptions::default()
        };
        let probe = probe_run(&scenario, &opts).expect("probe");
        let r_h = probe
            .transience
            .as_ref()
            .and_then(|t| t.r_h)
            .expect("precondition: heals");
        assert!(retraction_rounds(&scenario, probe.rounds).contains(&r_h));
        let row = probe_row("rq2", &scenario, &probe);
        let heals: u64 = column(&row, "heals").parse().expect("filled");
        assert!(heals >= 1);
        assert_eq!(
            column(&row, "heals_by_retraction"),
            heals.to_string(),
            "seed {seed}"
        );
        assert!(
            column(&row, "r_h_retractions")
                .parse::<u64>()
                .expect("filled")
                >= 1
        );
        assert!(
            column(&row, "retraction_rewrites")
                .parse::<u64>()
                .expect("filled")
                >= 1
        );
    }
}

#[test]
fn heals_count_episode_ends_and_the_ones_that_retracted() {
    let h = sim::persistence::heals(&[true, false, true, true, false], &[3, 2, 0, 0, 0]);
    assert_eq!(
        (h.heals, h.by_retraction, h.r_h_retractions),
        (2, 1, Some(2))
    );
    let open = sim::persistence::heals(&[true, true], &[1, 1]);
    assert_eq!(
        (open.heals, open.by_retraction, open.r_h_retractions),
        (0, 0, None)
    );
}

#[test]
fn a_heal_is_credited_to_a_retraction_anywhere_after_its_split_began() {
    let early = sim::persistence::heals(&[true, true, false], &[0, 4, 0]);
    assert_eq!(
        (early.heals, early.by_retraction, early.r_h_retractions),
        (1, 1, Some(0)),
        "the retraction one round before the heal round counts"
    );
    let at_start = sim::persistence::heals(&[true, false], &[5, 0]);
    assert_eq!(
        at_start.by_retraction, 0,
        "a retraction in the round the split appeared did not heal it"
    );
    let two = sim::persistence::heals(&[true, true, true, false, true, false], &[9, 1, 0, 0, 0, 0]);
    assert_eq!(
        (two.heals, two.by_retraction),
        (2, 1),
        "each episode is credited only with its own retractions"
    );
}
