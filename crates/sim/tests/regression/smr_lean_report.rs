//! `run_smr_lean` releases each command's spread curve when its window
//! closes. That is only sound because the grid CSV and its ledger row read
//! landmark fields alone — so the two runs must agree everywhere except the
//! curves themselves. This pins that: same landmarks, same CSV, curves gone.

use sim::smr::{
    ClientModel, CommandReport, CommandStatus, ManualBlock, Proto, SmrReport, SmrScenario,
    TrafficPhase, point_mass_pmf, run_smr, run_smr_lean,
};
use sim::sweep::{AnySpec, parse_specs, run_smr_grid_streaming};
use sim::{BlockSchedule, BlockTarget, BlockWindow, Config};
use std::collections::BTreeSet;

/// Loaded compact under blocking: ⊥ requesters, state adoption, commands
/// settling every round — the shape whose curves dominate tracker memory.
fn cell(max_rounds: usize, rate: usize) -> SmrScenario {
    SmrScenario {
        n: 32,
        seed: 975_300,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Compact {
            t_commit_rounds: 12,
        },
        injections: vec![],
        max_rounds,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(rate),
        }]),
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

/// The shed contract for one command, and whether the lean run released its
/// curve. Attempts, receivers and curve go together and only at settlement:
/// a released tracker must carry the full settled signature, or an early shed
/// would hide here. Anything still live has to match the full run exactly.
fn curve_released(f: &CommandReport, l: &CommandReport) -> bool {
    if l.attempts.is_empty() && !f.attempts.is_empty() {
        assert_eq!(
            l.status,
            CommandStatus::Complete,
            "released but not settled"
        );
        assert!(
            l.spread.points().is_empty(),
            "a released command's window is shut"
        );
        assert!(l.amp_receivers.is_empty(), "receivers go with the attempts");
    } else {
        assert_eq!(f.attempts, l.attempts, "a live command keeps its attempts");
        assert_eq!(f.amp_receivers, l.amp_receivers);
    }
    if l.spread.points().is_empty() && !f.spread.points().is_empty() {
        return true;
    }
    assert_eq!(f.spread, l.spread, "an unsettled curve must survive");
    false
}

#[test]
fn the_lean_run_keeps_every_landmark_and_drops_only_settled_curves() {
    let s = cell(120, 4);
    let full = run_smr(&s);
    let lean = run_smr_lean(&s);

    assert_eq!(full.terminal, lean.terminal);
    assert_eq!(full.safety_ok, lean.safety_ok);
    assert_eq!(full.commands.len(), lean.commands.len());

    let mut released = 0;
    for (f, l) in full.commands.iter().zip(&lean.commands) {
        assert_eq!(f.client, l.client);
        assert_eq!(f.op, l.op);
        assert_eq!(f.status, l.status);
        assert_eq!(f.injection_round, l.injection_round);
        assert_eq!(f.delivered_round, l.delivered_round);
        assert_eq!(f.all_logs_round, l.all_logs_round);
        assert_eq!(f.prefix_fixed_round, l.prefix_fixed_round);
        assert_eq!(f.committed_ack_round, l.committed_ack_round);
        assert_eq!(f.executed_round, l.executed_round);
        released += usize::from(curve_released(f, l));
    }
    assert!(released > 0, "the cell must settle commands to release any");
}

/// Everything the report carries and the lean run may not move: every
/// landmark, plus the `SpreadPoint`s of any curve that survives.
fn same_landmarks_and_surviving_curves(full: &SmrReport, lean: &SmrReport) -> usize {
    assert_eq!(full.terminal, lean.terminal);
    assert_eq!(full.safety_ok, lean.safety_ok);
    assert_eq!(full.metrics, lean.metrics, "per-round counters moved");
    assert_eq!(full.commands.len(), lean.commands.len());
    let mut released = 0;
    for (f, l) in full.commands.iter().zip(&lean.commands) {
        assert_eq!(
            (
                f.client,
                f.op,
                f.status,
                f.injection_round,
                f.delivered_round,
                f.all_logs_round,
                f.prefix_fixed_round,
                f.committed_ack_round,
                f.executed_round,
            ),
            (
                l.client,
                l.op,
                l.status,
                l.injection_round,
                l.delivered_round,
                l.all_logs_round,
                l.prefix_fixed_round,
                l.committed_ack_round,
                l.executed_round,
            )
        );
        released += usize::from(curve_released(f, l));
    }
    released
}

/// The lean run also has its servers forget the committed prefix of their
/// shared state. Two shapes the frontier has to survive: ⊥ servers stopping
/// short of the longest executed sequence, and a server blocked for the whole
/// horizon, frozen below the frontier and clamped at its own length.
#[test]
fn forgetting_the_committed_prefix_moves_no_landmark_on_either_shape() {
    for frozen in [false, true] {
        let mut s = cell(200, 4);
        // High enough that the cell does not split its brain: the pin is about
        // agreement on a run whose safety latch holds.
        s.proto = Proto::Compact {
            t_commit_rounds: 38,
        };
        if frozen {
            s.manual_blocks = vec![ManualBlock {
                node: 3,
                from_round: 1,
                to_round: None,
            }];
        }
        let full = run_smr(&s);
        let lean = run_smr_lean(&s);
        assert!(full.safety_ok, "frozen {frozen}: the cell split its brain");
        assert!(
            full.metrics
                .iter()
                .any(|m| m.min_executed_len < m.max_executed_len),
            "frozen {frozen}: the executed lengths never spread"
        );
        let released = same_landmarks_and_surviving_curves(&full, &lean);
        assert!(released > 0, "frozen {frozen}: nothing settled");
    }
}

/// Lean forgetting drops the positions a later shrink would re-verify, so the verdict a forking
/// cell gets must still match the full run: the safety latch and the first double execution.
#[test]
fn lean_and_full_agree_on_the_verdict_of_forking_cells() {
    let split: Vec<u32> = (0..32).map(|i| u32::from(i >= 24)).collect();
    let mut forked = 0;
    for seed in 0..6 {
        for kind in 0..3 {
            let mut s = cell(150, 3);
            s.seed = 880_000 + seed;
            s.proto = Proto::Compact { t_commit_rounds: 3 };
            match kind {
                0 => s.schedule = BlockSchedule::Permanent { fraction: 0.25 },
                1 => s.schedule = BlockSchedule::FreshPerRound { fraction: 0.3 },
                _ => {
                    s.schedule = BlockSchedule::FreshPerRound { fraction: 0.0 };
                    s.partition = Some(split.clone());
                }
            }
            let (full, lean) = (run_smr(&s), run_smr_lean(&s));
            assert_eq!(
                (full.safety_ok, full.exec_dup_round),
                (lean.safety_ok, lean.exec_dup_round),
                "seed {seed}, schedule {kind}"
            );
            forked += usize::from(!full.safety_ok);
        }
    }
    assert!(forked >= 6, "only {forked} of 18 cells forked");
}

/// The grid path takes the lean run. Rows still assemble — the CSV's own
/// byte-identity against committed evidence is the release byte-diff suite's
/// job, which this only has to not break.
#[test]
fn the_grid_still_assembles_rows_on_the_lean_run() {
    let spec_text = r#"[{
      "exp": "lean-pin",
      "base": {
        "n": 32, "seed": 0, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": { "kind": "compact", "t_commit_rounds": 12 },
        "injections": [], "max_rounds": 120,
        "schedule": { "kind": "fresh_per_round", "fraction": 0.0 }
      },
      "betas": [0.1], "rates": [4], "t_commits": [12],
      "ladder": [{ "n": 32, "seed_base": 975300, "seed_count": 3 }]
    }]"#;
    let specs = parse_specs(spec_text).expect("spec parses");
    let AnySpec::Smr(grid) = &specs[0] else {
        panic!("smr grid");
    };
    // An explicit ledger: the default target is the repo's committed
    // provenance file, which a test must never append to.
    let dir = std::env::temp_dir().join(format!("smr-lean-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let csv = run_smr_grid_streaming(grid, Some(&ledger), None).expect("grid runs");
    assert_eq!(csv.lines().count(), 4, "header + one row per seed");
    assert!(!csv.contains("NaN"));
}

/// The lean run also skips the per-round `spreading` list, which is
/// write-only on the grid path. Nothing the CSV reads may move: this pins the
/// exact projection `smr_row` takes.
///
/// It does NOT require a coverage-stranded command. Stranding — a command
/// executed and drained before it is ever in every live log at once, leaving
/// `all_logs_round` None forever — is the failure mode the skip was requested
/// for, but it does not occur at these parameters (coverage lands in a few
/// rounds, far inside `t_commit`), so a test asserting it would only ever
/// assert its own precondition.
#[test]
fn skipping_the_spreading_list_moves_nothing_the_grid_reads() {
    let s = cell(200, 4);
    let full = run_smr(&s);
    let lean = run_smr_lean(&s);

    let project = |r: &sim::smr::SmrReport| {
        (
            r.terminal,
            r.safety_ok,
            r.commands
                .iter()
                .map(|c| {
                    (
                        c.client,
                        c.op,
                        c.status,
                        c.injection_round,
                        c.prefix_fixed_round,
                        c.all_logs_round,
                        c.committed_ack_round,
                        c.executed_round,
                    )
                })
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(project(&full), project(&lean));
    assert!(
        full.commands
            .iter()
            .any(|c| c.status == CommandStatus::Complete),
        "a degenerate all-pending shape would pin nothing"
    );
}

// ── recovery ────────────────────────────────────────────────────────────────
//
// The grid hardwires `run_smr_lean` (`run_smr_grid_streaming`), so no runner
// path emits lean-off rows for a recovery arm and the release byte-identity
// battery can only ever compare lean against lean. The retention contract is
// witnessed here instead: per cell, `run_smr` against `run_smr_lean`, on shapes
// sampled from the recovery specs the campaign actually runs.

/// A recovery cell in the shape of the `amd6-flat` arm (n = 32, T = 20, rate 4,
/// unblocked). `straggler` holds node 0 out across a boundary and releases it,
/// which is the only shape that produces both an executed-length spread and a
/// mid-window checkpoint adoption on a run that still finishes: a stochastic
/// blocking fraction at this load drives recovery into its commitment-error
/// regime, where `end_window` aborts on `P must be a prefix of L` — a recorded
/// experiment outcome, and a run that produces no report cannot pin a report.
fn rec_cell(
    seed: u64,
    t_window: u64,
    rounds: usize,
    rate: usize,
    client_model: ClientModel,
    resend_until_acked: bool,
    straggler: Option<(usize, usize)>,
) -> SmrScenario {
    SmrScenario {
        n: 32,
        seed,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Recovery {
            t_window_rounds: t_window,
            resend_until_acked,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        injections: vec![],
        max_rounds: rounds,
        schedule: match straggler {
            Some((start_round, rounds)) => BlockSchedule::Windows(vec![BlockWindow {
                start_round,
                rounds,
                target: BlockTarget::Nodes(vec![0]),
            }]),
            None => BlockSchedule::FreshPerRound { fraction: 0.0 },
        },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(rate),
        }]),
        client_model,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

/// A straggler shape: node 0 blocked across the boundary at 120 and released
/// mid-window, so it misses a commit and can only catch up by adopting a newer
/// checkpoint — state changes mid-window only through adoption.
fn straggler_cell(
    seed: u64,
    rate: usize,
    client_model: ClientModel,
    resend_until_acked: bool,
) -> SmrScenario {
    rec_cell(
        seed,
        40,
        260,
        rate,
        client_model,
        resend_until_acked,
        Some((100, 51)),
    )
}

/// Did some node catch up to the longest executed sequence at a NON-boundary
/// round? Executed state moves mid-window only by checkpoint adoption, so this
/// is the adoption witness the metrics rows can carry.
fn adopted_mid_window(report: &SmrReport, t_window: usize) -> bool {
    let m = &report.metrics;
    (1..m.len()).any(|i| {
        (i + 1) % t_window != 0
            && m[i - 1].min_executed_len < m[i - 1].max_executed_len
            && m[i].min_executed_len == m[i].max_executed_len
            && m[i].min_executed_len > m[i - 1].min_executed_len
    })
}

/// Recovery's settled signature is `executed_round` — §6 execution on every
/// useful server — not the client ack: a recovery client gets no ack unless the
/// arm runs `resend_until_acked`, so a shed keyed on the ack would never fire.
/// Under ack mode the per-round history is held until the ack lands, which is
/// the one place the two latches come apart.
fn rec_curve_released(f: &CommandReport, l: &CommandReport, ack_mode: bool) -> bool {
    if l.attempts.is_empty() && !f.attempts.is_empty() {
        assert!(
            l.executed_round.is_some(),
            "released but not §6-executed (op {})",
            l.op
        );
        assert_eq!(
            l.status,
            CommandStatus::Complete,
            "released but not settled"
        );
        assert!(
            l.spread.points().is_empty(),
            "a released command's window is shut"
        );
        assert!(l.amp_receivers.is_empty(), "receivers go with the attempts");
        assert!(
            !ack_mode || l.committed_ack_round.is_some(),
            "under ack mode the client stage is still writing until the ack (op {})",
            l.op
        );
    } else {
        assert_eq!(f.attempts, l.attempts, "a live command keeps its attempts");
        assert_eq!(f.amp_receivers, l.amp_receivers);
    }
    if l.spread.points().is_empty() && !f.spread.points().is_empty() {
        return true;
    }
    assert_eq!(f.spread, l.spread, "an unsettled curve must survive");
    false
}

/// Every field the recovery report carries, compared between the two runners.
/// `fork_ok` is in here deliberately: it is a grid CSV column fed from the
/// lineage oracle, the grid is lean-only, and an oracle reading whole histories
/// would either go blind or manufacture forks once a node forgets.
fn rec_same_report(scenario: &SmrScenario, label: &str) -> usize {
    let full = run_smr(scenario);
    let lean = run_smr_lean(scenario);
    let ack_mode = matches!(
        scenario.proto,
        Proto::Recovery {
            resend_until_acked: true,
            ..
        }
    );

    assert_eq!(full.terminal, lean.terminal, "{label}: terminal");
    assert_eq!(full.safety_ok, lean.safety_ok, "{label}: safety latch");
    assert_eq!(
        full.metrics, lean.metrics,
        "{label}: per-round counters moved"
    );
    assert_eq!(
        full.pool_peak_in_flight, lean.pool_peak_in_flight,
        "{label}: pool occupancy moved — an observer change reached the client stage"
    );
    let (f_rec, l_rec) = (
        full.recovery.as_ref().expect("recovery report"),
        lean.recovery.as_ref().expect("recovery report"),
    );
    assert_eq!(
        f_rec.fork_ok, l_rec.fork_ok,
        "{label}: the lineage oracle moved"
    );
    assert_eq!(
        f_rec.rounds, l_rec.rounds,
        "{label}: a recovery counter row moved"
    );
    assert_eq!(
        full.commands.len(),
        lean.commands.len(),
        "{label}: commands"
    );

    let mut released = 0;
    for (f, l) in full.commands.iter().zip(&lean.commands) {
        assert_eq!(
            (
                f.client,
                f.op,
                f.status,
                f.injection_round,
                f.delivered_round,
                f.all_logs_round,
                f.prefix_fixed_round,
                f.committed_ack_round,
                f.executed_round,
            ),
            (
                l.client,
                l.op,
                l.status,
                l.injection_round,
                l.delivered_round,
                l.all_logs_round,
                l.prefix_fixed_round,
                l.committed_ack_round,
                l.executed_round,
            ),
            "{label}: landmark moved on op {}",
            f.op
        );
        released += usize::from(rec_curve_released(f, l, ack_mode));
    }
    assert!(
        full.commands.iter().any(|c| c.executed_round.is_some()),
        "{label}: nothing committed — the cell would pin its own precondition"
    );
    released
}

/// The horizon has to cross several T-window boundaries, or a cell witnesses
/// nothing about a forget that only ever fires at one. `needs_spread` is set
/// wherever blocking should leave some server behind: an unblocked cell keeps
/// every executed sequence the same length by construction, so demanding a
/// spread there would only assert that the arm is not the arm it is.
fn assert_shape_bites(scenario: &SmrScenario, label: &str, needs_spread: bool) {
    let report = run_smr(scenario);
    let rec = report.recovery.as_ref().expect("recovery report");
    let windows: BTreeSet<u64> = rec.rounds.iter().map(|r| r.max_checkpoint_window).collect();
    assert!(
        windows.len() >= 4,
        "{label}: only {} checkpoint windows — too few boundaries to forget at",
        windows.len()
    );
    assert!(
        !needs_spread
            || report
                .metrics
                .iter()
                .any(|m| m.min_executed_len < m.max_executed_len),
        "{label}: the executed lengths never spread"
    );
}

/// Sampled recovery grid cells: the unblocked `amd6-flat` shape, a blocked one
/// where the executed lengths spread, and a short window that commits often.
/// Each must produce the same report on both runners.
#[test]
fn recovery_lean_matches_full_on_sampled_grid_cells() {
    let cells = [
        (
            "amd6-flat shape",
            rec_cell(4_340_000, 20, 200, 4, ClientModel::Unique, false, None),
            false,
        ),
        (
            "long horizon: the release line travels far from the origin",
            rec_cell(4_340_020, 20, 400, 4, ClientModel::Unique, false, None),
            false,
        ),
        (
            "straggler: executed lengths spread, then close by adoption",
            straggler_cell(4_340_024, 4, ClientModel::Unique, false),
            true,
        ),
    ];
    for (label, scenario, needs_spread) in &cells {
        assert_shape_bites(scenario, label, *needs_spread);
        let released = rec_same_report(scenario, label);
        assert!(
            released > 0,
            "{label}: nothing settled, so nothing was released"
        );
    }
}

/// Observation is not passive in the recovery engine: the boundary pass's
/// `executed_round` frees a pool slot, and under `resend_until_acked` the
/// `committed_ack_round` latch gates client resends. Both feed the client
/// stage, which draws from the RNG — so an observer that read a forgotten
/// prefix wrongly would move the stream, not merely a metric.
#[test]
fn recovery_lean_matches_full_across_the_client_feedback_channel() {
    // A recovery command settles two T-windows after arrival, so the pool has
    // to cover a whole commitment pipeline; an undersized one aborts on
    // exhaustion instead of exercising the reuse path.
    let cells = [
        (
            "pool: executed_round frees a slot",
            rec_cell(
                4_340_001,
                20,
                200,
                2,
                ClientModel::Pool { clients: 256 },
                false,
                None,
            ),
            false,
        ),
        (
            "pool across an adoption: slots free while a peer catches up",
            straggler_cell(4_340_021, 2, ClientModel::Pool { clients: 256 }, false),
            true,
        ),
        (
            "ack mode: committed_ack_round gates resends",
            rec_cell(4_340_002, 20, 200, 2, ClientModel::Unique, true, None),
            false,
        ),
        (
            "ack mode across an adoption",
            straggler_cell(4_340_022, 2, ClientModel::Unique, true),
            true,
        ),
    ];
    for (label, scenario, needs_spread) in &cells {
        assert_shape_bites(scenario, label, *needs_spread);
        rec_same_report(scenario, label);
        // The channel has to be live, or the cell pins nothing about it.
        let report = run_smr_lean(scenario);
        match scenario.client_model {
            ClientModel::Pool { .. } => assert!(
                report.pool_peak_in_flight.is_some_and(|p| p > 1),
                "{label}: the pool never had two commands in flight"
            ),
            ClientModel::Unique => assert!(
                report
                    .commands
                    .iter()
                    .any(|c| c.committed_ack_round.is_some()),
                "{label}: no command was ever acked"
            ),
        }
    }
}

/// A command still pending when a boundary fires, and one still pending while a
/// straggler adopts, are the two shapes where the boundary pass reads a node
/// that just committed and a node that just took someone else's state
/// wholesale. The adopted state carries the DONOR's forget line as its offset,
/// which is exactly the equal-length/different-offset pair that a fork
/// signature reading `SharedState`'s own equality would assert on.
#[test]
fn recovery_lean_matches_full_with_commands_pending_across_an_adoption() {
    let label = "pending across an adoption";
    let scenario = straggler_cell(4_340_023, 4, ClientModel::Unique, false);
    assert_shape_bites(&scenario, label, true);
    let full = run_smr(&scenario);
    assert!(
        adopted_mid_window(&full, 40),
        "{label}: no node caught up mid-window, so no adoption was exercised"
    );
    assert!(
        full.commands
            .iter()
            .any(|c| c.executed_round.is_none() && c.delivered_round.is_some()),
        "{label}: no command stayed pending — the boundary pass never saw one"
    );
    rec_same_report(&scenario, label);
}

/// A node's OFFSET is not its forget line, and the observer release line has
/// to respect the difference.
///
/// A rollback restores `self.checkpoint.state` and an adoption restores a
/// peer's — and a checkpoint's state carries the offset it had when it was
/// MINTED, which is an older line than the node has since forgotten to.
/// Genesis carries offset 0. So offsets go BACKWARDS, and a release line
/// derived only from checkpoint LENGTHS will sit above an offset some node is
/// later restored to, at which point the checker's shrink path re-verifies
/// from a position the observer no longer retains.
///
/// This is the `E9-beta01` shape from the campaign grid — quiet for three
/// T-windows, then a 60% sticky surge over a 10% background, which strands
/// most of the population on old checkpoints while the rest keeps committing.
/// It is the cell that caught the bug: on a release line derived from
/// checkpoint LENGTHS alone it aborts with "offset 0 is below the retained
/// canonical order (start 2)".
#[test]
fn a_surge_that_strands_old_checkpoints_still_forgets_soundly() {
    // The quiet-then-surge fraction ladder, exactly as the committed spec
    // writes it: 120 rounds at 0.0, then 80 at 0.6 (rounds past the ladder
    // take 0.0 again), over a 0.1 background.
    let mut fractions: Vec<String> = vec!["0.0".into(); 120];
    fractions.extend(std::iter::repeat_n("0.6".to_string(), 80));
    let spec_text = format!(
        r#"[{{
          "exp": "surge-strands",
          "base": {{
            "n": 128, "seed": 0, "k": 6, "ell": 3, "sigma": 1.0,
            "proto": {{ "kind": "recovery", "t_window_rounds": 40, "resend_until_acked": false }},
            "injections": [
              {{ "round": 5, "client": 1, "op": 7 }},
              {{ "round": 45, "client": 2, "op": 9 }},
              {{ "round": 330, "client": 3, "op": 11 }}
            ],
            "max_rounds": 440,
            "schedule": {{
              "kind": "per_round_sticky",
              "fractions": [{}],
              "background": 0.1
            }}
          }},
          "ladder": [{{ "n": 128, "seed_base": 3100000, "seed_count": 20 }}]
        }}]"#,
        fractions.join(",")
    );
    let specs = parse_specs(&spec_text).expect("spec parses");
    let AnySpec::Smr(grid) = &specs[0] else {
        panic!("smr grid");
    };
    let dir = std::env::temp_dir().join(format!("smr-surge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let csv =
        run_smr_grid_streaming(grid, Some(&dir.join("ledger.jsonl")), None).expect("the grid runs");
    assert_eq!(csv.lines().count(), 21, "header + one row per seed");
    // The surge has to actually bite, or the cell pins nothing: ⊥ servers are
    // what strand old checkpoints in the population.
    assert!(
        csv.lines().skip(1).any(|row| row.contains(",true,")),
        "no row reported a live safety latch — the shape did not run as expected"
    );
}

/// R9: the lab can never run lean. `sim-wasm` builds an `SmrState` directly
/// (`LiveSmr::new_with_mode`), so the only thing between the browser and a
/// forgotten history is that these two fields are unreachable from another
/// crate. Today that is an accident of where they were declared; here it is a
/// stated contract.
#[test]
fn the_lean_flags_stay_crate_private() {
    let source = include_str!("../../src/smr.rs");
    for field in ["drop_settled_spread", "truncate_history", "observe_spread"] {
        assert!(
            source.contains(&format!("pub(crate) {field}: bool,")),
            "SmrState::{field} must stay `pub(crate)` — a downstream crate \
             (sim-wasm above all) could otherwise turn lean on, and the live \
             lab has no spill to reconstruct a forgotten history from"
        );
        assert!(
            !source.contains(&format!("pub {field}: bool,")),
            "SmrState::{field} is public"
        );
    }
}
