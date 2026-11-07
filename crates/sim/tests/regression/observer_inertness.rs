//! Observer inertness: the simulated world (`smr/engine/`) must behave identically whether
//! the optional observation over it (`smr/observe/`) runs or not. One seeded scenario runs
//! four ways — every observer on, lean with spill, grid-lean with the census scoped out, and
//! flat (grid that retires settled commands and folds rows as they land) —
//! and the per-round protocol state of every node plus every RNG-dependent landmark must
//! agree. An observer that consumed an RNG draw or wrote world state would shift the logs
//! within a round or two.
//!
//! The narrower pins stay where they are: `smr_lean_report` (lean == full on reports),
//! `smr_census_scope` (census on == off on reports), `smr_cert_gating` (certs on == off).
//! This file is the combined guarantee, compared on node state rather than on reports.

use protocol::compact::{Entry, SharedState, Timed};
use protocol::recovery::RState;
use sim::smr::{
    ClientModel, CommandReport, Proto, SmrReport, SmrScenario, SmrState, SmrSummary, TrafficPhase,
    grid_observes_census, point_mass_pmf, run_smr_flat_observed, run_smr_grid_observed,
    run_smr_lean_observed, run_smr_observed,
};
use sim::{BlockSchedule, BlockTarget, BlockWindow, Config};
use std::hint::black_box;

#[derive(Debug, PartialEq)]
struct Executed {
    offset: u64,
    len: u64,
    retained: Vec<Entry>,
    sn: Vec<(u32, u64)>,
}

impl Executed {
    fn of(state: &SharedState) -> Self {
        let view = state.executed();
        Executed {
            offset: view.offset(),
            len: view.logical_len(),
            retained: view.iter_from(view.offset()).cloned().collect(),
            sn: state.sn_iter().collect(),
        }
    }
}

#[derive(Debug)]
struct RecoveryTrace {
    reset: RState,
    checkpoint_window: u64,
    checkpoint_pre: Option<Vec<Timed>>,
    checkpoint_state: Executed,
}

#[derive(Debug)]
struct NodeTrace {
    log: Option<Vec<Timed>>,
    executed: Executed,
    recovery: Option<RecoveryTrace>,
}

type Trace = Vec<Vec<NodeTrace>>;

fn node_trace(state: &SmrState, i: usize) -> NodeTrace {
    let executed = Executed::of(state.node_state(i).expect("compact or recovery"));
    match state.recovery_node(i) {
        Some(node) => {
            let cp = node.checkpoint();
            NodeTrace {
                log: node.log_seq().map(|l| l.to_vec()),
                executed,
                recovery: Some(RecoveryTrace {
                    reset: node.reset_state(),
                    checkpoint_window: cp.w,
                    checkpoint_pre: cp.p.clone(),
                    checkpoint_state: Executed::of(&cp.s),
                }),
            }
        }
        None => NodeTrace {
            log: state.compact_logs()[i].clone(),
            executed,
            recovery: None,
        },
    }
}

fn record(state: &SmrState, n: usize, trace: &mut Trace) {
    assert_eq!(state.round(), trace.len() + 1, "one observation per round");
    trace.push((0..n).map(|i| node_trace(state, i)).collect());
}

/// Every read-only accessor a driver or the lab may call mid-run.
fn observe_everything(state: &SmrState, n: usize) {
    black_box(state.memory_accounting());
    black_box(state.report());
    black_box(state.executed_seqs());
    black_box(state.committed_entries());
    black_box(state.compact_logs());
    for i in 0..n {
        black_box(state.rec_node_detail(i));
        black_box(state.checkpoint_certs(i));
    }
}

struct Arm {
    label: &'static str,
    report: SmrReport,
    trace: Trace,
}

/// Full history, census on, §5 certificates mounted on recovery, every accessor each round.
fn all_on(scenario: &SmrScenario) -> Arm {
    let mut on = scenario.clone();
    on.certs = matches!(on.proto, Proto::Recovery { .. });
    let mut trace = Trace::new();
    let report = run_smr_observed(&on, &mut |state| {
        observe_everything(state, on.n);
        record(state, on.n, &mut trace);
    });
    Arm {
        label: "all observers on",
        report,
        trace,
    }
}

fn lean(scenario: &SmrScenario) -> Arm {
    let mut trace = Trace::new();
    let report =
        run_smr_lean_observed(scenario, &mut |state| record(state, scenario.n, &mut trace));
    Arm {
        label: "lean (forgetting, spill)",
        report,
        trace,
    }
}

/// The census is scoped out here unless the process runs with `SIM_CENSUS=on`.
fn grid(scenario: &SmrScenario) -> Arm {
    let mut trace = Trace::new();
    let report =
        run_smr_grid_observed(scenario, &mut |state| record(state, scenario.n, &mut trace));
    Arm {
        label: "grid (lean, census scoped out)",
        report,
        trace,
    }
}

/// The flat arm has no report; its node trace, its summary and how many commands it retired.
fn flat(scenario: &SmrScenario) -> (Trace, SmrSummary, usize) {
    let mut trace = Trace::new();
    let mut retired = 0;
    let summary = run_smr_flat_observed(scenario, &mut |state| {
        record(state, scenario.n, &mut trace);
        retired = state.command_residency().0;
    });
    (trace, summary, retired)
}

fn assert_executed_agrees(reference: &Executed, got: &Executed, at: &str) {
    assert_eq!(
        reference.offset, 0,
        "{at}: the reference arm forgot a prefix"
    );
    assert_eq!(got.len, reference.len, "{at}: executed length");
    assert_eq!(got.sn, reference.sn, "{at}: committed sequence numbers");
    assert!(
        got.retained[..] == reference.retained[got.offset as usize..],
        "{at}: a retained executed entry differs"
    );
}

fn assert_log_agrees(reference: &Option<Vec<Timed>>, got: &Option<Vec<Timed>>, at: &str) {
    match (reference, got) {
        (Some(r), Some(g)) => {
            let first = r.iter().zip(g).position(|(a, b)| a != b);
            assert!(
                first.is_none() && r.len() == g.len(),
                "{at}: log differs (lengths {} vs {}, first differing position {first:?})",
                r.len(),
                g.len()
            );
        }
        _ => assert_eq!(got.is_some(), reference.is_some(), "{at}: log ⊥"),
    }
}

fn assert_node_agrees(reference: &NodeTrace, got: &NodeTrace, at: &str) {
    assert_log_agrees(&reference.log, &got.log, at);
    assert_executed_agrees(&reference.executed, &got.executed, at);
    match (&reference.recovery, &got.recovery) {
        (None, None) => {}
        (Some(r), Some(g)) => {
            assert_eq!(g.reset, r.reset, "{at}: R");
            assert_eq!(
                g.checkpoint_window, r.checkpoint_window,
                "{at}: checkpoint W"
            );
            assert_eq!(g.checkpoint_pre, r.checkpoint_pre, "{at}: checkpoint P");
            assert_executed_agrees(
                &r.checkpoint_state,
                &g.checkpoint_state,
                &format!("{at} checkpoint S"),
            );
        }
        _ => panic!("{at}: engine kind differs"),
    }
}

/// Landmarks and the RNG-drawn targets; the lean path sheds a settled command's attempts.
fn assert_command_agrees(reference: &CommandReport, got: &CommandReport, at: &str) {
    assert_eq!(
        (
            got.client,
            got.op,
            got.injection_round,
            got.auto,
            got.status
        ),
        (
            reference.client,
            reference.op,
            reference.injection_round,
            reference.auto,
            reference.status
        ),
        "{at}: identity"
    );
    assert_eq!(
        (
            got.delivered_round,
            got.all_logs_round,
            got.prefix_fixed_round,
            got.committed_ack_round,
            got.executed_round
        ),
        (
            reference.delivered_round,
            reference.all_logs_round,
            reference.prefix_fixed_round,
            reference.committed_ack_round,
            reference.executed_round
        ),
        "{at}: landmarks"
    );
    if !got.attempts.is_empty() {
        assert_eq!(got.attempts, reference.attempts, "{at}: delivery attempts");
        assert_eq!(
            got.amp_receivers, reference.amp_receivers,
            "{at}: amplify targets"
        );
    }
}

fn assert_arm_agrees(reference: &Arm, got: &Arm) {
    let label = got.label;
    assert_eq!(
        got.trace.len(),
        reference.trace.len(),
        "{label}: rounds run"
    );
    for (r, (ref_nodes, got_nodes)) in reference.trace.iter().zip(&got.trace).enumerate() {
        for (i, (rn, gn)) in ref_nodes.iter().zip(got_nodes).enumerate() {
            assert_node_agrees(rn, gn, &format!("{label}, round {}, node {i}", r + 1));
        }
    }
    let (a, b) = (&reference.report, &got.report);
    assert_eq!(b.terminal, a.terminal, "{label}: terminal");
    assert_eq!(b.safety_ok, a.safety_ok, "{label}: safety latch");
    assert_eq!(b.failure, a.failure, "{label}: failure");
    assert_eq!(b.metrics, a.metrics, "{label}: per-round metrics rows");
    assert_eq!(b.recovery, a.recovery, "{label}: recovery rows and fork_ok");
    assert_eq!(
        b.pool_peak_in_flight, a.pool_peak_in_flight,
        "{label}: pool peak"
    );
    assert_eq!(b.commands.len(), a.commands.len(), "{label}: command count");
    for (rc, gc) in a.commands.iter().zip(&b.commands) {
        assert_command_agrees(rc, gc, &format!("{label}, op {}", rc.op));
    }
}

/// Returns the reference arm so callers can check the shape exercised what it claims to.
fn assert_observers_inert(scenario: &SmrScenario) -> Arm {
    let reference = all_on(scenario);
    let lean = lean(scenario);
    let grid = grid(scenario);
    assert!(
        lean.trace
            .iter()
            .flatten()
            .any(|node| node.executed.offset > 0),
        "the lean arm never forgot a prefix, so forgetting was not exercised"
    );
    assert!(
        reference
            .report
            .commands
            .iter()
            .any(|c| c.spread.observed().is_some()),
        "the reference arm did not observe the census"
    );
    if !grid_observes_census() {
        assert!(
            grid.report
                .commands
                .iter()
                .all(|c| c.spread.observed().is_none()),
            "the grid arm observed the census it should scope out"
        );
    }
    let spill_opted_out = ["SIM_SPILL", "SIM_RUNLOG"]
        .iter()
        .any(|v| std::env::var(v).as_deref() == Ok("off"));
    if matches!(scenario.proto, Proto::Recovery { .. }) && !spill_opted_out {
        assert!(
            lean.report.spill_path.is_some(),
            "the lean recovery arm wrote no spill"
        );
    }
    assert_arm_agrees(&reference, &lean);
    assert_arm_agrees(&reference, &grid);

    let (trace, mut summary, retired) = flat(scenario);
    assert!(
        retired > 0,
        "the flat arm retired no command, so retirement was not exercised"
    );
    assert_arm_agrees(
        &reference,
        &Arm {
            label: "flat (retiring, rows folded)",
            report: reference.report.clone(),
            trace,
        },
    );
    summary.spill_path = None;
    assert_eq!(
        summary,
        SmrSummary::from_report(&reference.report, scenario),
        "flat: summary"
    );
    reference
}

fn compact_cell() -> SmrScenario {
    SmrScenario {
        n: 32,
        seed: 975_300,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Compact {
            t_commit_rounds: 38,
        },
        injections: vec![],
        max_rounds: 150,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(4),
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

/// Node 0 is held out across the boundary at 120 and released mid-window, so it catches up
/// by adopting a newer checkpoint.
fn recovery_cell(seed: u64, client_model: ClientModel, resend_until_acked: bool) -> SmrScenario {
    SmrScenario {
        n: 32,
        seed,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Recovery {
            t_window_rounds: 40,
            resend_until_acked,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        injections: vec![],
        max_rounds: 260,
        schedule: BlockSchedule::Windows(vec![BlockWindow {
            start_round: 100,
            rounds: 51,
            target: BlockTarget::Nodes(vec![0]),
        }]),
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(2),
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

#[test]
fn observers_are_inert_on_compact() {
    let reference = assert_observers_inert(&compact_cell());
    assert!(
        reference
            .report
            .commands
            .iter()
            .any(|c| c.committed_ack_round.is_some()),
        "no command was ever acked"
    );
}

/// `executed_round` is observer-derived and frees a pool slot, whose reuse draws from the RNG.
#[test]
fn observers_are_inert_on_recovery_through_the_pool_channel() {
    let scenario = recovery_cell(4_340_021, ClientModel::Pool { clients: 256 }, false);
    let reference = assert_observers_inert(&scenario);
    assert!(
        reference.report.pool_peak_in_flight.is_some_and(|p| p > 1),
        "the pool never had two commands in flight"
    );
    assert!(
        reference
            .report
            .commands
            .iter()
            .any(|c| c.executed_round.is_some()),
        "no command executed, so no slot was freed"
    );
}

/// Under `resend_until_acked` the ack latch gates client resends, each drawing a target.
#[test]
fn observers_are_inert_on_recovery_through_the_ack_channel() {
    let scenario = recovery_cell(4_340_022, ClientModel::Unique, true);
    let reference = assert_observers_inert(&scenario);
    assert!(
        reference
            .report
            .commands
            .iter()
            .any(|c| c.committed_ack_round.is_some()),
        "no command was ever acked"
    );
}
