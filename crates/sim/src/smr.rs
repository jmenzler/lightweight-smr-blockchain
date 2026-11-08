//! SMR drivers: round loops for the extended, compact and recovery engines.
//!
//! `engine` is the simulated world (RNG draws, node steps), `observe` the read-only
//! measurement over it. `tracker` is both: a command tracker holds the client's retry state
//! that stage (b) reads and the landmarks the observation pass writes.

mod engine;
mod observe;
mod summary;
mod tracker;
mod types;
mod validate;

pub use crate::runlog::{run_smr_logged, run_smr_recorded};
pub use engine::engine_rec::boundary_key_counts;
pub(crate) use engine::pool::EXHAUSTED_MARKER as POOL_EXHAUSTED_MARKER;
pub use observe::accounting::{Accounting, Footprint, NodeBytes, cert_state_bytes, footprint};
pub use observe::census_count::census_counts;
pub use observe::detail::RecNodeDetail;
pub use observe::full_prefix::UnsettledAge;
pub use observe::log_index::holder_vec_count;
pub use observe::safety::{EntrySeq, PrefixChecker, prefixes_consistent};
pub use observe::spill::{Spill, SpillBoundary, SpillHeader, SpillTrailer, read_spill};
pub use summary::{CommandFold, LatencySamples, RecoverySummary, SmrSummary};
#[cfg(test)]
pub(crate) use summary::{client_end_round, paper_latency_sample};
pub use tracker::injection_sns;
pub use types::{
    AUTO_CLIENT_BASE, AUTO_OP_BASE, AttemptOutcome, ClientModel, CommandReport, CommandStatus,
    DeliveryAttempt, HaltKind, Injection, ManualBlock, MergePolicy, NodeGlance, PrefixMismatch,
    Proto, RecRoundMetrics, RecStepInfo, RecoveryReport, RecoveryViolation, RepeatedCommit,
    RunFailure, RunFailurePhase, SEED_COMMAND, SmrReport, SmrRoundMetrics, SmrScenario,
    SmrStepStatus, SmrTerminal, SpreadCurve, SpreadPoint, SpreadingCommand, TrafficPhase,
    active_phase, amp_count, auto_command,
};
pub(crate) use types::{PrefixMismatchWire, RepeatedCommitWire, is_abort, is_execute};
pub use validate::point_mass_pmf;
pub(crate) use validate::validate_weights;

use crate::blocking::{
    adjust_sticky_mask, checked_mask, compile_history_only, fraction_count, history_only_view,
    sample_blocked, sticky_with_background,
};
use crate::{Config, same_component};
use engine::engine_rec::RecRunState;
use engine::pool::ClientPool;
use observe::boundary_skip::BoundarySkipTally;
use observe::entry_index::EntryIndex;
use observe::exec_dup::ExecDupOracle;
use protocol::LogNode;
use protocol::certificates::ServerCertState;
use protocol::compact::{ClientCommand, CompactNode, Entry};
use protocol::recovery::RecoveryNode;
use protocol::shared_state::ExecSeq;
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use tracker::{CommandTracker, trackers_for};
use validate::{validate_injection, validate_pmf};

fn par_step_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SIM_PAR_STEP").as_deref() == Ok("on"))
}

fn census_forced_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SIM_CENSUS").as_deref() == Ok("on"))
}

/// Run an SMR scenario to its horizon; panics on an invalid scenario or a violated invariant.
pub fn run_smr(scenario: &SmrScenario) -> SmrReport {
    run_smr_inner(scenario, RunScope::full())
}

#[derive(Debug, Clone, Copy)]
struct RunScope {
    lean: bool,
    observe_spread: bool,
    flat: bool,
}

impl RunScope {
    fn full() -> Self {
        RunScope {
            lean: false,
            observe_spread: true,
            flat: false,
        }
    }

    fn lean() -> Self {
        RunScope {
            lean: true,
            observe_spread: true,
            flat: false,
        }
    }

    fn grid() -> Self {
        RunScope {
            lean: true,
            observe_spread: census_forced_on(),
            flat: false,
        }
    }

    fn flat() -> Self {
        RunScope {
            flat: true,
            ..RunScope::grid()
        }
    }
}

/// As [`run_smr`], but releases settled spread curves and forgets committed prefixes.
pub fn run_smr_lean(scenario: &SmrScenario) -> SmrReport {
    run_smr_inner(scenario, RunScope::lean())
}

/// The grid's runner: lean, with the position census scoped out.
pub fn run_smr_grid(scenario: &SmrScenario) -> SmrReport {
    run_smr_inner(scenario, RunScope::grid())
}

pub fn grid_observes_census() -> bool {
    census_forced_on()
}

/// The sweep's runner: [`run_smr_grid`] that also retires every command no stage can change any
/// more and folds per-round rows as they land, so only live commands and per-client protocol
/// state grow. Returns what a sweep row and its ledger record read, not a report.
pub fn run_smr_flat(scenario: &SmrScenario) -> SmrSummary {
    run_smr_flat_observed(scenario, &mut |_: &SmrState| {})
}

/// [`run_smr_flat`] with the live state after every round.
pub fn run_smr_flat_observed(
    scenario: &SmrScenario,
    observe: &mut dyn FnMut(&SmrState),
) -> SmrSummary {
    run_loop(scenario, RunScope::flat(), observe).into_summary()
}

/// [`run_smr_grid`] with the live state after every round.
pub fn run_smr_grid_observed(
    scenario: &SmrScenario,
    observe: &mut dyn FnMut(&SmrState),
) -> SmrReport {
    run_smr_sampled(scenario, RunScope::grid(), observe)
}

/// Whether [`run_smr_lean`] has the servers forget their committed prefix.
pub const LEAN_FORGETS_HISTORY: bool = true;

/// [`run_smr_lean`] with the live state after every round; the observer only reads.
pub fn run_smr_lean_observed(
    scenario: &SmrScenario,
    observe: &mut dyn FnMut(&SmrState),
) -> SmrReport {
    run_smr_sampled(scenario, RunScope::lean(), observe)
}

/// [`run_smr`] with the live state after every round.
pub fn run_smr_observed(scenario: &SmrScenario, observe: &mut dyn FnMut(&SmrState)) -> SmrReport {
    run_smr_sampled(scenario, RunScope::full(), observe)
}

fn run_smr_inner(scenario: &SmrScenario, scope: RunScope) -> SmrReport {
    run_smr_sampled(scenario, scope, &mut |_: &SmrState| {})
}

fn run_smr_sampled(
    scenario: &SmrScenario,
    scope: RunScope,
    observe: &mut dyn FnMut(&SmrState),
) -> SmrReport {
    let state = run_loop(scenario, scope, observe);
    let report = state.report();
    debug_assert_eq!(
        state.summary(),
        SmrSummary::from_report(&report, scenario),
        "the streamed summary differs from the one read off the report"
    );
    report
}

fn run_loop(
    scenario: &SmrScenario,
    scope: RunScope,
    observe: &mut dyn FnMut(&SmrState),
) -> SmrState {
    scenario.validate().expect("invalid SmrScenario");
    let mut state = SmrState::new_with_certs(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &scenario.injections,
        scenario.traffic.as_deref().unwrap_or(&[]),
        scenario.client_model,
        scenario.partition.as_deref(),
        scenario.certs,
        scenario.merge_policy,
        scenario.repeated_commit,
    );
    state.drop_settled_spread = scope.lean;
    state.truncate_history = scope.lean;
    state.observe_spread = scope.observe_spread;
    state.flat = scope.flat;
    state.scenario_tag = Some(crate::runlog::params_tag(scenario));
    let mut blocking = compile_history_only(&scenario.schedule, scenario.n, state.rng_mut());
    let progress = crate::progress::cell(scenario.n, scenario.seed, scenario.max_rounds);
    for round in 1..=scenario.max_rounds {
        progress.tick(round);
        state.draw_arrivals();
        let mut mask = checked_mask(
            blocking.as_mut(),
            &history_only_view(round),
            state.rng_mut(),
            scenario.n,
        );
        for mb in &scenario.manual_blocks {
            if mb.covers(round) {
                mask[mb.node as usize] = true;
            }
        }
        let status = state.step_masked(&mask);
        if status.failed() {
            break;
        }
        observe(&state);
        if status.dead.is_some() {
            break;
        }
        if scenario.halt_on_violation && !state.safety_ok() {
            state.halt = Some((state.round, HaltKind::Fork));
            break;
        }
    }
    progress.done();
    if state.failure.is_none() {
        state.finish_spill();
    }
    state
}

enum SmrNodes {
    Ext(Vec<LogNode>),
    Comp(Vec<CompactNode>),
    Rec(Vec<RecoveryNode>),
}

/// Incremental SMR stepping core; stage order is the RNG contract (sim/smr-driver-architecture.md).
pub struct SmrState {
    n: usize,
    seed: u64,
    cfg: Config,
    proto: Proto,
    amp: usize,
    rng: ChaCha12Rng,
    nodes: SmrNodes,
    trackers: Vec<CommandTracker>,
    client_order: Vec<usize>,
    metrics: Vec<SmrRoundMetrics>,
    round: usize,
    dead: Option<usize>,
    failure: Option<RunFailure>,
    failed_status: Option<SmrStepStatus>,
    safety_ok: bool,
    checker: PrefixChecker,
    exec_dup: ExecDupOracle,
    committed: observe::committed::CommittedOracle,
    /// Entries the execute-once state machine left out, over all nodes, and the first such round.
    repeat_skips: u64,
    first_repeat_skip_round: Option<usize>,
    boundary_skips: BoundarySkipTally,
    wedged_round: Option<usize>,
    /// Set by the run loop when `halt_on_violation` stops it.
    halt: Option<(usize, HaltKind)>,
    canon_index: EntryIndex,
    inert_prefix: usize,
    /// Compact: trackers behind the inert prefix whose commit is not dated yet.
    undated: Vec<usize>,
    pub(crate) drop_settled_spread: bool,
    pub(crate) truncate_history: bool,
    pub(crate) observe_spread: bool,
    /// Live-only T_E over executed part ++ log (compact, recovery); see `observe_full_prefix`.
    full_prefix: bool,
    /// Per round while `full_prefix` is on: the oldest unsettled age, `None` once safety is lost.
    unsettled_ages: Vec<Option<UnsettledAge>>,
    /// Retire settled commands and fold per-round rows; the run reports a summary only.
    flat: bool,
    retired: summary::CommandFold,
    row_fold: summary::RowFold,
    frontier: u64,
    release_line: u64,
    spill: Option<observe::spill::SpillWriter>,
    spill_allowed: bool,
    /// Tag of the full scenario, set by the scenario runner; names the spill file.
    pub(crate) scenario_tag: Option<String>,
    traffic: Vec<TrafficPhase>,
    pool: Option<ClientPool>,
    auto_spawned: u64,
    arrivals_drawn_for: Option<usize>,
    arrivals_pending: u32,
    rec: Option<RecRunState>,
    #[cfg(test)]
    rec_boundary_snapshot: Option<Vec<engine::engine_rec::RecBoundaryNodeSnapshot>>,
    partition: Option<Vec<u32>>,
}

impl SmrState {
    /// Draws nothing after seeding: node constructors are RNG-free.
    // Every parameter explicit: a driver must name its client model and partition.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        n: usize,
        cfg: Config,
        proto: Proto,
        sigma: f64,
        seed: u64,
        injections: &[Injection],
        traffic: &[TrafficPhase],
        client_model: ClientModel,
        partition: Option<&[u32]>,
    ) -> SmrState {
        SmrState::new_with_certs(
            n,
            cfg,
            proto,
            sigma,
            seed,
            injections,
            traffic,
            client_model,
            partition,
            false,
            MergePolicy::default(),
            RepeatedCommit::Execute,
        )
    }

    /// As [`SmrState::new`], mounting the §5 certificate layer on recovery nodes when `certs` is set.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_certs(
        n: usize,
        cfg: Config,
        proto: Proto,
        sigma: f64,
        seed: u64,
        injections: &[Injection],
        traffic: &[TrafficPhase],
        client_model: ClientModel,
        partition: Option<&[u32]>,
        certs: bool,
        merge_policy: MergePolicy,
        repeated_commit: RepeatedCommit,
    ) -> SmrState {
        crate::check_partition(partition, n).expect("invalid partition");
        let rng = ChaCha12Rng::seed_from_u64(seed);
        let mut rec = None;
        let nodes = match proto {
            Proto::Extended => {
                SmrNodes::Ext((0..n).map(|_| LogNode::new(SEED_COMMAND, cfg)).collect())
            }
            Proto::Compact { t_commit_rounds } => SmrNodes::Comp(
                (0..n)
                    .map(|_| {
                        CompactNode::new(cfg, t_commit_rounds)
                            .with_merge_policy(merge_policy.stamp_tie, merge_policy.union_stamp)
                            .with_repeated_commit(repeated_commit)
                    })
                    .collect(),
            ),
            Proto::Recovery {
                t_window_rounds,
                resend_until_acked,
                prefix_mismatch,
            } => {
                rec = Some(RecRunState::new(
                    t_window_rounds,
                    n,
                    resend_until_acked,
                    prefix_mismatch,
                ));
                SmrNodes::Rec(
                    (0..n)
                        .map(|_| {
                            RecoveryNode::new(cfg, t_window_rounds, certs)
                                .with_merge_policy(merge_policy.stamp_tie, merge_policy.union_stamp)
                                .with_prefix_mismatch(prefix_mismatch)
                                .with_repeated_commit(repeated_commit)
                        })
                        .collect(),
                )
            }
        };
        let (trackers, client_order) = trackers_for(injections);
        SmrState {
            n,
            seed,
            cfg,
            proto,
            amp: amp_count(sigma, n),
            rng,
            nodes,
            trackers,
            client_order,
            metrics: Vec::new(),
            round: 0,
            dead: None,
            failure: None,
            failed_status: None,
            safety_ok: true,
            checker: PrefixChecker::new(n),
            exec_dup: ExecDupOracle::new(n),
            committed: observe::committed::CommittedOracle::new(n),
            repeat_skips: 0,
            first_repeat_skip_round: None,
            boundary_skips: BoundarySkipTally::new(n),
            wedged_round: None,
            halt: None,
            canon_index: EntryIndex::new(),
            inert_prefix: 0,
            undated: Vec::new(),
            drop_settled_spread: false,
            truncate_history: false,
            observe_spread: true,
            full_prefix: false,
            unsettled_ages: Vec::new(),
            flat: false,
            retired: summary::CommandFold::default(),
            row_fold: summary::RowFold::default(),
            frontier: 0,
            release_line: 0,
            spill: None,
            spill_allowed: matches!(proto, Proto::Recovery { .. })
                && observe::spill::spill_enabled(),
            scenario_tag: None,
            traffic: traffic.to_vec(),
            pool: client_model.pool_size().map(ClientPool::new),
            auto_spawned: 0,
            arrivals_drawn_for: None,
            arrivals_pending: 0,
            rec,
            #[cfg(test)]
            rec_boundary_snapshot: None,
            partition: partition.map(<[u32]>::to_vec),
        }
    }

    /// Stage (a0): exactly one weighted arrival draw per round, before any mask draw.
    pub fn draw_arrivals(&mut self) {
        self.assert_not_failed("draw arrivals");
        let round = self.round + 1;
        assert!(
            self.arrivals_drawn_for != Some(round),
            "draw_arrivals called twice for round {round}"
        );
        self.arrivals_drawn_for = Some(round);
        let Some(phase) = active_phase(&self.traffic, round) else {
            return;
        };
        let count = crate::draw_weighted_index(&mut self.rng, &phase.arrivals_pmf);
        self.arrivals_pending = count as u32;
        for _ in 0..count {
            let j = self.auto_spawned;
            self.auto_spawned += 1;
            let cc = match &mut self.pool {
                None => auto_command(j),
                Some(pool) => {
                    let (client, sn) = pool.take(&mut self.rng, round);
                    ClientCommand {
                        client,
                        sn,
                        op: AUTO_OP_BASE + j,
                    }
                }
            };
            self.trackers.push(CommandTracker {
                client: cc.client,
                op: cc.op,
                cc,
                injection_round: round,
                target: None,
                auto: true,
                injected_round: None,
                delivered_round: None,
                all_logs_round: None,
                prefix_fixed_round: None,
                index_agreed_round: None,
                full_prefix_fixed_round: None,
                agreed_index: None,
                committed_ack_round: None,
                committed_round: None,
                executed_round: None,
                fixed_prefix: None,
                spread: Vec::new(),
                spread_points: 0,
                attempts: Vec::new(),
                amp_receivers: Vec::new(),
                canon_cursor: None,
            });
        }
        if count > 0 {
            self.client_order = (0..self.trackers.len()).collect();
            self.client_order.sort_by_key(|&i| self.trackers[i].client);
        }
    }

    /// Fresh-style step: arrivals, then this round's blocked set, then the round.
    pub fn step_sampled(&mut self, count: usize) -> SmrStepStatus {
        self.assert_not_failed("sample and step");
        if !self.traffic.is_empty() && self.arrivals_drawn_for != Some(self.round + 1) {
            self.draw_arrivals();
        }
        let mask = sample_blocked(&mut self.rng, self.n, count);
        self.step_masked(&mask)
    }

    pub fn step_fraction(&mut self, fraction: f64) -> SmrStepStatus {
        self.step_sampled(self.blocked_count(fraction))
    }

    /// Largest blocked-set size whose realized fraction stays within budget.
    pub fn blocked_count(&self, fraction: f64) -> usize {
        fraction_count(fraction, self.n)
    }

    pub fn sample_mask(&mut self, count: usize) -> Vec<bool> {
        self.assert_not_failed("sample a mask");
        self.assert_arrivals_before_masks();
        sample_blocked(&mut self.rng, self.n, count)
    }

    pub fn adjust_sticky(&mut self, mask: &mut [bool], count: usize) {
        self.assert_not_failed("adjust a sticky mask");
        self.assert_arrivals_before_masks();
        adjust_sticky_mask(&mut self.rng, mask, count, self.n);
    }

    /// The batch driver's sticky round mask: adjust `sticky` to `count`, then union a background sample.
    pub fn sticky_round_mask(
        &mut self,
        sticky: &mut [bool],
        count: usize,
        background_count: usize,
    ) -> Vec<bool> {
        self.assert_not_failed("draw a sticky mask");
        self.assert_arrivals_before_masks();
        sticky_with_background(&mut self.rng, sticky, count, background_count, self.n)
    }

    fn assert_arrivals_before_masks(&self) {
        assert!(
            self.traffic.is_empty() || self.arrivals_drawn_for == Some(self.round + 1),
            "traffic configured: draw_arrivals must precede mask draws for round {}",
            self.round + 1
        );
    }

    /// Live pmf edit: lands from the next round and replaces every later phase.
    pub fn set_traffic_pmf(&mut self, pmf: Vec<f64>) -> Result<(), String> {
        if self.failure.is_some() {
            return Err("run failed; traffic cannot be changed".into());
        }
        validate_pmf(&pmf)?;
        let from_round = self.round + 1;
        self.traffic.retain(|p| p.from_round < from_round);
        self.traffic.push(TrafficPhase {
            from_round,
            arrivals_pmf: pmf,
        });
        Ok(())
    }

    /// The recorded phase list (ctor phases + live edits), the replay export source.
    pub fn traffic_phases(&self) -> &[TrafficPhase] {
        &self.traffic
    }

    /// Manual commands only; a replay regenerates traffic arrivals from phases + seed.
    pub fn manual_injections(&self) -> Vec<Injection> {
        self.assert_records_kept("list manual injections");
        self.trackers
            .iter()
            .filter(|t| !t.auto)
            .map(|t| Injection {
                round: t.injection_round,
                client: t.client,
                op: t.op,
                target: t.target,
            })
            .collect()
    }

    pub(crate) fn rng_mut(&mut self) -> &mut ChaCha12Rng {
        self.assert_not_failed("draw from the RNG");
        &mut self.rng
    }

    fn assert_not_failed(&self, action: &str) {
        assert!(self.failure.is_none(), "run failed; cannot {action}");
    }

    /// A flat run has dropped settled commands and past rows; nothing may answer as if it had not.
    fn assert_records_kept(&self, action: &str) {
        assert!(
            !self.flat,
            "a flat run keeps no settled commands or past rows; cannot {action} (read summary())"
        );
    }

    pub fn round(&self) -> usize {
        self.round
    }

    pub fn failure(&self) -> Option<&RunFailure> {
        self.failure.as_ref()
    }

    /// Register a live command; it lands next round, since stamping this round would break replay.
    /// Refused while its client has a command scheduled later: its sn would differ on replay.
    pub fn inject(&mut self, client: u32, op: u64, target: Option<u32>) -> Result<usize, String> {
        if self.failure.is_some() {
            return Err("run failed; commands cannot be injected".into());
        }
        self.assert_records_kept("inject a command");
        let duplicate = self.trackers.iter().any(|t| t.op == op);
        validate_injection(client, op, target, self.n, duplicate)?;
        let round = self.round + 1;
        if let Some(later) = self
            .trackers
            .iter()
            .find(|t| !t.auto && t.client == client && t.injection_round > round)
        {
            return Err(format!(
                "client {client} already has op {} scheduled for round {}; sequence numbers \
                 follow injection rounds, so a live command cannot precede it",
                later.op, later.injection_round
            ));
        }
        let (mut new_tracker, _) = trackers_for(&[Injection {
            round,
            client,
            op,
            target,
        }]);
        let mut tracker = new_tracker.pop().expect("one tracker");
        tracker.cc.sn = 1 + self
            .trackers
            .iter()
            .filter(|t| !t.auto && t.client == client)
            .count() as u64;
        self.trackers.push(tracker);
        self.client_order = (0..self.trackers.len()).collect();
        self.client_order.sort_by_key(|&i| self.trackers[i].client);
        Ok(round)
    }

    /// Each command's wire identity and auto flag, in issue (tracker) order from index `from` on.
    pub fn issued_from(&self, from: usize) -> impl Iterator<Item = (ClientCommand, bool)> + '_ {
        self.assert_records_kept("list issued commands");
        self.trackers[from..].iter().map(|t| (t.cc, t.auto))
    }

    /// Every command this session knows, in tracker order, for replay export.
    pub fn injections(&self) -> Vec<Injection> {
        self.assert_records_kept("list injections");
        self.trackers
            .iter()
            .map(|t| Injection {
                round: t.injection_round,
                client: t.client,
                op: t.op,
                target: t.target,
            })
            .collect()
    }

    /// Step under a precomputed mask; callers must stop on `dead` or diverge from batch replays.
    pub fn step_masked(&mut self, mask: &[bool]) -> SmrStepStatus {
        if let Some(status) = &self.failed_status {
            return status.clone();
        }
        assert!(
            self.traffic.is_empty() || self.arrivals_drawn_for == Some(self.round + 1),
            "traffic configured but draw_arrivals not called for round {}",
            self.round + 1
        );
        self.arrivals_drawn_for = None;
        self.round += 1;
        let status = match &self.nodes {
            SmrNodes::Ext(_) => self.step_ext(mask),
            SmrNodes::Comp(_) => self.step_comp(mask),
            SmrNodes::Rec(_) => self.step_rec(mask),
        };
        self.fold_rows();
        if self.flat && self.failure.is_none() {
            self.retire_settled();
            self.prune_lineage();
        }
        self.tally_repeat_skips();
        if let SmrNodes::Rec(nodes) = &self.nodes {
            let stale = nodes.iter().map(RecoveryNode::stale_checkpoint);
            self.boundary_skips.observe(stale, self.round);
            let wedged = observe::fleet_wedge::fleet_wedged(nodes);
            assert!(
                self.wedged_round.is_none() || wedged,
                "round {}: the fleet left a wedge declared at round {:?}, which no step can leave",
                self.round,
                self.wedged_round
            );
            if wedged {
                self.wedged_round.get_or_insert(self.round);
            }
        }
        status
    }

    fn tally_repeat_skips(&mut self) {
        let total = match &self.nodes {
            SmrNodes::Ext(_) => 0,
            SmrNodes::Comp(nodes) => nodes.iter().map(CompactNode::repeat_skips).sum(),
            SmrNodes::Rec(nodes) => nodes.iter().map(RecoveryNode::repeat_skips).sum(),
        };
        if total > self.repeat_skips && self.first_repeat_skip_round.is_none() {
            self.first_repeat_skip_round = Some(self.round);
        }
        self.repeat_skips = total;
    }

    /// Which servers hold L_i = ⊥ now.
    pub fn bottom_logs(&self) -> Vec<bool> {
        self.assert_not_failed("read live log presence");
        match &self.nodes {
            SmrNodes::Ext(nodes) => nodes.iter().map(|nd| nd.log().is_none()).collect(),
            SmrNodes::Comp(nodes) => nodes.iter().map(|nd| nd.log_seq().is_none()).collect(),
            SmrNodes::Rec(nodes) => nodes.iter().map(|nd| nd.log_seq().is_none()).collect(),
        }
    }

    /// Per-node executed sequences (compact only).
    pub fn executed_seqs(&self) -> Vec<Vec<Entry>> {
        self.assert_not_failed("read live executed sequences");
        match &self.nodes {
            SmrNodes::Comp(nodes) => nodes
                .iter()
                .map(|nd| nd.shared_state().untruncated().to_vec())
                .collect(),
            SmrNodes::Rec(nodes) => nodes
                .iter()
                .map(|nd| nd.shared_state().untruncated().to_vec())
                .collect(),
            SmrNodes::Ext(_) => Vec::new(),
        }
    }

    /// One node's state; unlike [`Self::executed_entries`] it does not panic on a truncated state.
    pub fn node_state(&self, node: usize) -> Option<&protocol::compact::SharedState> {
        self.assert_not_failed("read live node state");
        match &self.nodes {
            SmrNodes::Comp(nodes) => Some(nodes[node].shared_state()),
            SmrNodes::Rec(nodes) => Some(nodes[node].shared_state()),
            SmrNodes::Ext(_) => None,
        }
    }

    /// One node's executed prefix, uncloned; panics on a truncated state.
    pub fn executed_entries(&self, node: usize) -> Option<&ExecSeq> {
        self.assert_not_failed("read live executed entries");
        match &self.nodes {
            SmrNodes::Comp(nodes) => Some(nodes[node].shared_state().untruncated()),
            SmrNodes::Rec(nodes) => Some(nodes[node].shared_state().untruncated()),
            SmrNodes::Ext(_) => None,
        }
    }

    /// The longest executed sequence over all n recovery nodes (⊥ nodes included), ties by lowest id.
    /// A display construct: §6 defines a server's committed commands by its own S_i (p. 35), and
    /// while prefix consistency holds this sequence extends every server's S_i.
    pub fn committed_entries(&self) -> Option<Vec<Entry>> {
        self.assert_not_failed("read live committed entries");
        let SmrNodes::Rec(nodes) = &self.nodes else {
            return None;
        };
        Some(
            nodes
                .iter()
                .map(|nd| nd.shared_state().untruncated())
                .enumerate()
                .max_by_key(|(i, executed)| (executed.len(), std::cmp::Reverse(*i)))
                .map(|(_, executed)| executed.to_vec())
                .unwrap_or_default(),
        )
    }

    /// One node's checkpoint-resident §5 certificate state (recovery with certs only).
    pub fn checkpoint_certs(&self, node: usize) -> Option<&ServerCertState> {
        self.assert_not_failed("read live checkpoint certificates");
        let SmrNodes::Rec(nodes) = &self.nodes else {
            return None;
        };
        nodes.get(node)?.checkpoint().certs.as_ref()
    }

    /// One recovery node, read-only; unlike [`Self::rec_node_detail`] safe on a lean run.
    pub fn recovery_node(&self, node: usize) -> Option<&RecoveryNode> {
        self.assert_not_failed("read a live recovery node");
        let SmrNodes::Rec(nodes) = &self.nodes else {
            return None;
        };
        nodes.get(node)
    }

    pub fn rec_node_detail(&self, node: usize) -> Option<RecNodeDetail> {
        self.assert_not_failed("read live recovery node detail");
        let SmrNodes::Rec(nodes) = &self.nodes else {
            return None;
        };
        let nd = nodes.get(node)?;
        let checkpoint = nd.checkpoint();
        Some(RecNodeDetail {
            node: node as u32,
            r: engine::engine_rec::r_code(nd.reset_state()),
            log_len: nd.log_seq().map(|l| l.len() as u32),
            executed_len: nd.shared_state().logical_len() as u32,
            checkpoint_window: checkpoint.w,
            checkpoint_p_len: checkpoint.p.as_ref().map(|p| p.len() as u32),
            s_hash: observe::detail::hash_hex(&nd.shared_state().untruncated().to_vec()),
            checkpoint_s_hash: observe::detail::hash_hex(&checkpoint.s.untruncated().to_vec()),
        })
    }

    pub fn compact_logs(&self) -> Vec<Option<Vec<protocol::compact::Timed>>> {
        self.assert_not_failed("read live compact logs");
        match &self.nodes {
            SmrNodes::Comp(nodes) => nodes.iter().map(CompactNode::log_entries).collect(),
            SmrNodes::Ext(_) | SmrNodes::Rec(_) => Vec::new(),
        }
    }

    /// Live sessions only: date compact and recovery `prefix_fixed_round` over each server's
    /// executed part ++ log, and record every round's oldest unsettled age. Batch drivers never
    /// call it, so their reports stay byte-identical.
    pub fn observe_full_prefix(&mut self) {
        assert_eq!(
            self.round, 0,
            "the observer has to see every round from the first"
        );
        assert!(
            !self.truncate_history && !self.flat,
            "lean and flat runs forget the canonical order this observer reads"
        );
        self.full_prefix = true;
    }

    /// The oldest unsettled ages (full and contested) per round while `observe_full_prefix` is on.
    pub fn unsettled_ages(&self) -> &[Option<UnsettledAge>] {
        &self.unsettled_ages
    }

    /// The report's `safety_ok` no-fork latch without rebuilding the report.
    pub fn safety_ok(&self) -> bool {
        self.safety_ok
    }

    /// False once a command was executed twice (compact or recovery); always true for extended.
    pub fn exec_unique_ok(&self) -> bool {
        self.exec_dup.tripped_round().is_none()
    }

    /// Def 1.5 retractions so far, per server and observation: (all, the rewrites among them).
    pub fn retraction_counts(&self) -> (u64, u64) {
        self.committed.retractions()
    }

    /// Folds the rows this step pushed; a flat run then drops them.
    fn fold_rows(&mut self) {
        let from = if self.flat { 0 } else { self.row_fold.rounds() };
        let rec = self.rec.as_ref();
        if let Some(rec) = rec {
            assert_eq!(
                rec.rows.len(),
                self.metrics.len(),
                "one recovery row per round"
            );
        }
        for (i, m) in self.metrics.iter().enumerate().skip(from) {
            let row = rec.map(|r| (&r.rows[i], self.n, r.t_window()));
            self.row_fold.add(m, row);
        }
        assert_eq!(
            self.row_fold.rounds(),
            self.round,
            "one row per stepped round"
        );
        if self.flat {
            self.metrics.clear();
            if let Some(rec) = &mut self.rec {
                rec.rows.clear();
            }
        }
    }

    /// Lineage keys below every node's checkpoint window can never be looked up again.
    fn prune_lineage(&mut self) {
        if let (Some(rec), SmrNodes::Rec(nodes)) = (&mut self.rec, &self.nodes) {
            let lowest = nodes.iter().map(|nd| nd.checkpoint().w).min();
            rec.prune_lineage_below(lowest.expect("at least one node"));
        }
    }

    /// Commands a flat run has retired, and commands still held.
    pub fn command_residency(&self) -> (usize, usize) {
        (self.retired.commands, self.trackers.len())
    }

    /// What a sweep row and its ledger record read; the only output of a flat run.
    pub fn summary(&self) -> SmrSummary {
        self.summary_with(self.retired.clone())
    }

    /// [`Self::summary`] without copying the retired samples, the run's largest remaining term.
    fn into_summary(mut self) -> SmrSummary {
        let retired = std::mem::take(&mut self.retired);
        self.summary_with(retired)
    }

    fn summary_with(&self, retired: summary::CommandFold) -> SmrSummary {
        SmrSummary::streamed(
            self.report_records(false),
            self.proto,
            retired,
            &self.trackers,
            &self.row_fold,
        )
    }

    /// The report so far — non-consuming, callable mid-session; panics on a flat run.
    pub fn report(&self) -> SmrReport {
        self.assert_records_kept("build a report");
        self.report_records(true)
    }

    /// With `records` false, the report's scalars alone: no commands, rows or recovery rounds.
    fn report_records(&self, records: bool) -> SmrReport {
        let terminal = if let Some(failure) = &self.failure {
            SmrTerminal::Failed {
                attempted_round: failure.attempted_round,
                completed_round: failure.completed_round,
                observed_round: failure.observed_round,
                phase: failure.phase,
            }
        } else {
            match (self.dead, self.halt) {
                (Some(round), _) => SmrTerminal::Dead { round },
                (None, Some((round, kind))) => SmrTerminal::Halted { round, kind },
                (None, None) => SmrTerminal::Ran { rounds: self.round },
            }
        };
        let mut commands: Vec<CommandReport> = if records {
            self.trackers
                .iter()
                .map(|t| t.report(terminal, self.observe_spread))
                .collect()
        } else {
            Vec::new()
        };
        // (injection_round, client) is total and shared by a live session and its replay.
        commands.sort_by_key(|c| (c.injection_round, c.client));
        SmrReport {
            metrics: if records {
                self.metrics.clone()
            } else {
                Vec::new()
            },
            commands,
            terminal,
            safety_ok: self.safety_ok,
            exec_dup_round: self.exec_dup.tripped_round(),
            retractions: self.committed.retractions().0,
            retraction_rewrites: self.committed.retractions().1,
            first_retraction_round: self.committed.first_retraction_round(),
            strong_safety_violation_rounds: self.committed.strong_safety().0,
            strong_safety_first_violation: self.committed.strong_safety().1,
            strong_safety_boundary_violations: self.committed.strong_safety_at_boundaries().0,
            strong_safety_boundary_first_violation: self.committed.strong_safety_at_boundaries().1,
            repeat_skips: self.repeat_skips,
            first_repeat_skip_round: self.first_repeat_skip_round,
            boundary_skip_events: self.boundary_skips.events(),
            first_boundary_skip_round: self.boundary_skips.first_round(),
            last_boundary_skip_round: self.boundary_skips.last_round(),
            max_rejoin_rounds: self.boundary_skips.max_rejoin_rounds(),
            longest_stale_rounds: self.boundary_skips.longest_stale_rounds(),
            unrejoined_nodes: self.boundary_skips.unrejoined(),
            horizon_skip_nodes: self.boundary_skips.horizon_skips(),
            wedged_round: self.wedged_round,
            failure: self.failure.clone(),
            recovery: self.rec.as_ref().map(|r| RecoveryReport {
                fork_ok: r.fork_ok,
                commit_fork_round: r.commit_fork_round,
                rounds: if records { r.rows.clone() } else { Vec::new() },
            }),
            pool_peak_in_flight: self.pool.as_ref().map(ClientPool::peak_in_flight),
            spill_path: self
                .spill
                .as_ref()
                .map(|s| s.path().to_string_lossy().into_owned()),
        }
    }

    pub(crate) fn finish_spill(&mut self) {
        if let Some(spill) = &mut self.spill
            && let Err(cause) = spill.finish()
        {
            spill_io_failure("trailer write", self.seed, Some(spill.path()), &cause);
        }
    }
}

/// A run whose canonical stream cannot be written is unauditable, like an unlogged ledger row:
/// stop the sweep, name the run and the way out, and record no failure row for an I/O error.
pub(crate) fn spill_io_failure(
    step: &str,
    seed: u64,
    path: Option<&std::path::Path>,
    cause: &str,
) -> ! {
    let file = path.map_or(String::new(), |p| format!(" ({})", p.display()));
    panic!(
        "spill {step} failed for seed {seed}{file}: {cause}. The run cannot be audited without \
         its canonical stream; fix the disk or set SIM_SPILL=off to run without it."
    )
}

fn blocked_ids(mask: &[bool]) -> Vec<u32> {
    mask.iter()
        .enumerate()
        .filter_map(|(i, &b)| b.then_some(i as u32))
        .collect()
}

#[cfg(test)]
mod tests;
