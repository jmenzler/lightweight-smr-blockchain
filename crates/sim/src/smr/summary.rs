//! What a sweep row and its ledger record read off a run, built two ways: from a full
//! `SmrReport`, or streamed by a flat run that drops each command once nothing can change it.

use super::tracker::CommandTracker;
use super::{
    CommandReport, CommandStatus, Proto, RecRoundMetrics, RunFailure, SmrReport, SmrRoundMetrics,
    SmrScenario, SmrTerminal,
};

/// The landmarks the two latency families read.
struct Landmarks {
    injection_round: usize,
    injected_round: Option<usize>,
    prefix_fixed_round: Option<usize>,
    committed_ack_round: Option<usize>,
    committed_round: Option<usize>,
    executed_round: Option<usize>,
}

impl Landmarks {
    fn of_report(c: &CommandReport) -> Self {
        Landmarks {
            injection_round: c.injection_round,
            injected_round: c.injected_round,
            prefix_fixed_round: c.prefix_fixed_round,
            committed_ack_round: c.committed_ack_round,
            committed_round: c.committed_round,
            executed_round: c.executed_round,
        }
    }

    fn of_tracker(t: &CommandTracker) -> Self {
        Landmarks {
            injection_round: t.injection_round,
            injected_round: t.injected_round,
            prefix_fixed_round: t.prefix_fixed_round,
            committed_ack_round: t.committed_ack_round,
            committed_round: t.committed_round,
            executed_round: t.executed_round,
        }
    }

    /// Where the proto's client stops resending.
    fn client_end(&self, proto: Proto) -> Option<usize> {
        match proto {
            Proto::Extended => self.prefix_fixed_round,
            Proto::Compact { .. }
            | Proto::Recovery {
                resend_until_acked: true,
                ..
            } => self.committed_ack_round,
            Proto::Recovery {
                resend_until_acked: false,
                ..
            } => self.executed_round,
        }
    }

    // Lemma 3.8 prefix fixed (Extended; not Def 1.3's index agreement), commitment on all servers
    // with L_i != ⊥ (Compact, §1.2), execution on all servers with L_i != ⊥ (Recovery, Lemma 6.8).
    fn paper_end(&self, proto: Proto) -> Option<usize> {
        match proto {
            Proto::Extended => self.prefix_fixed_round,
            Proto::Compact { .. } => self.committed_round,
            Proto::Recovery { .. } => self.executed_round,
        }
    }
}

#[cfg(test)]
pub(crate) fn client_end_round(proto: Proto, c: &CommandReport) -> Option<usize> {
    Landmarks::of_report(c).client_end(proto)
}

/// (injection, paper end landmark); `None` for a command no non-blocked server ever received.
#[cfg(test)]
pub(crate) fn paper_latency_sample(
    proto: Proto,
    c: &CommandReport,
) -> Option<(usize, Option<usize>)> {
    let marks = Landmarks::of_report(c);
    marks.injected_round.map(|r| (r, marks.paper_end(proto)))
}

/// (origin, end) latency samples, 8 bytes each; `UNSETTLED` stands for no end.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LatencySamples(Vec<(u32, u32)>);

const UNSETTLED: u32 = u32::MAX;

fn round_u32(round: usize) -> u32 {
    u32::try_from(round)
        .ok()
        .filter(|&r| r != UNSETTLED)
        .unwrap_or_else(|| panic!("round {round} does not fit a latency sample"))
}

impl LatencySamples {
    fn push(&mut self, origin: usize, end: Option<usize>) {
        self.0
            .push((round_u32(origin), end.map_or(UNSETTLED, round_u32)));
    }

    /// Order carries no meaning: every statistic over the samples is a multiset statistic.
    fn canonicalize(&mut self) {
        self.0.sort_unstable();
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, Option<usize>)> + Clone + '_ {
        self.0
            .iter()
            .map(|&(o, e)| (o as usize, (e != UNSETTLED).then_some(e as usize)))
    }

    pub fn heap_bytes(&self) -> u64 {
        (self.0.capacity() * size_of::<(u32, u32)>()) as u64
    }
}

/// The per-command half of a summary: status counts and both latency families.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandFold {
    pub commands: usize,
    pub complete: usize,
    pub pending: usize,
    pub dead: usize,
    /// From `injected_round` to the paper landmark; injected commands only.
    pub paper: LatencySamples,
    /// From `injection_round` to where the client stops resending.
    pub client: LatencySamples,
}

impl CommandFold {
    fn add(&mut self, proto: Proto, marks: &Landmarks, status: CommandStatus) {
        self.commands += 1;
        match status {
            CommandStatus::Complete => self.complete += 1,
            CommandStatus::Pending => self.pending += 1,
            CommandStatus::Dead => self.dead += 1,
        }
        if let Some(origin) = marks.injected_round {
            self.paper.push(origin, marks.paper_end(proto));
        }
        self.client
            .push(marks.injection_round, marks.client_end(proto));
    }

    /// A tracker that no stage can change any more; its status is Complete whatever the terminal.
    pub(super) fn retire(&mut self, proto: Proto, t: &CommandTracker) {
        let status = t.status(SmrTerminal::Ran { rounds: 0 });
        assert_eq!(
            status,
            CommandStatus::Complete,
            "op {}: only a complete command can be retired",
            t.op
        );
        self.add(proto, &Landmarks::of_tracker(t), status);
    }

    pub fn heap_bytes(&self) -> u64 {
        self.paper.heap_bytes() + self.client.heap_bytes()
    }
}

/// The per-round half of a summary, folded row by row as each round's rows land.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RowFold {
    rounds: usize,
    min_useful: Option<u32>,
    rollbacks_total: u64,
    /// Earliest clean window end with `useful_agree` since the last unclean one.
    recovered_candidate: Option<usize>,
}

impl RowFold {
    /// The next round's rows; `rec` is (recovery row, n, T), on a recovery run only.
    pub(super) fn add(&mut self, m: &SmrRoundMetrics, rec: Option<(&RecRoundMetrics, usize, u64)>) {
        self.rounds += 1;
        let round = self.rounds;
        self.min_useful = Some(self.min_useful.map_or(m.useful, |u| u.min(m.useful)));
        let Some((row, n, t_window)) = rec else {
            return;
        };
        self.rollbacks_total += u64::from(row.rollbacks);
        if (round as u64).is_multiple_of(t_window) {
            let floor = (3 * n).div_ceil(4) as u32;
            let clean = row.reset == 0 && m.nonbot_logs >= floor;
            if !clean {
                self.recovered_candidate = None;
            } else if self.recovered_candidate.is_none() && row.useful_agree == Some(true) {
                self.recovered_candidate = Some(round);
            }
        }
    }

    pub(super) fn rounds(&self) -> usize {
        self.rounds
    }
}

/// The recovery block of a summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverySummary {
    pub fork_ok: bool,
    pub commit_fork_round: Option<usize>,
    pub rollbacks_total: u64,
    /// `SmrReport::recovered_round`.
    pub recovered_round: Option<usize>,
}

/// Every value the sweep CSV row and the run ledger read, and nothing per round or per command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmrSummary {
    pub terminal: SmrTerminal,
    pub safety_ok: bool,
    pub exec_dup_round: Option<usize>,
    pub retractions: u64,
    pub retraction_rewrites: u64,
    pub first_retraction_round: Option<usize>,
    pub strong_safety_violation_rounds: u64,
    pub strong_safety_first_violation: Option<usize>,
    pub strong_safety_boundary_violations: u64,
    pub strong_safety_boundary_first_violation: Option<usize>,
    pub repeat_skips: u64,
    pub first_repeat_skip_round: Option<usize>,
    pub boundary_skip_events: u64,
    pub first_boundary_skip_round: Option<usize>,
    pub last_boundary_skip_round: Option<usize>,
    pub max_rejoin_rounds: Option<usize>,
    pub longest_stale_rounds: Option<usize>,
    pub unrejoined_nodes: usize,
    pub horizon_skip_nodes: usize,
    pub wedged_round: Option<usize>,
    pub failure: Option<RunFailure>,
    pub recovery: Option<RecoverySummary>,
    pub pool_peak_in_flight: Option<u32>,
    pub spill_path: Option<String>,
    pub commands: CommandFold,
    /// Fewest useful servers in any round; `None` before the first round.
    pub min_useful: Option<u32>,
}

fn status_of(terminal: SmrTerminal, complete: bool) -> CommandStatus {
    if complete {
        CommandStatus::Complete
    } else if matches!(terminal, SmrTerminal::Dead { .. }) {
        CommandStatus::Dead
    } else {
        CommandStatus::Pending
    }
}

impl CommandTracker {
    pub(super) fn status(&self, terminal: SmrTerminal) -> CommandStatus {
        status_of(
            terminal,
            self.prefix_fixed_round.is_some()
                || self.committed_ack_round.is_some()
                || self.executed_round.is_some(),
        )
    }
}

impl SmrSummary {
    /// The reference construction, from a full report.
    pub fn from_report(report: &SmrReport, scenario: &SmrScenario) -> Self {
        let mut commands = CommandFold::default();
        for c in &report.commands {
            commands.add(scenario.proto, &Landmarks::of_report(c), c.status);
        }
        commands.paper.canonicalize();
        commands.client.canonicalize();
        let recovery = report.recovery.as_ref().map(|rec| RecoverySummary {
            fork_ok: rec.fork_ok,
            commit_fork_round: rec.commit_fork_round,
            rollbacks_total: rec.rounds.iter().map(|m| u64::from(m.rollbacks)).sum(),
            recovered_round: match scenario.proto {
                Proto::Recovery {
                    t_window_rounds, ..
                } => report.recovered_round(scenario.n, t_window_rounds),
                _ => unreachable!("a recovery block comes from a recovery run"),
            },
        });
        SmrSummary {
            terminal: report.terminal,
            safety_ok: report.safety_ok,
            exec_dup_round: report.exec_dup_round,
            retractions: report.retractions,
            retraction_rewrites: report.retraction_rewrites,
            first_retraction_round: report.first_retraction_round,
            strong_safety_violation_rounds: report.strong_safety_violation_rounds,
            strong_safety_first_violation: report.strong_safety_first_violation,
            strong_safety_boundary_violations: report.strong_safety_boundary_violations,
            strong_safety_boundary_first_violation: report.strong_safety_boundary_first_violation,
            repeat_skips: report.repeat_skips,
            first_repeat_skip_round: report.first_repeat_skip_round,
            boundary_skip_events: report.boundary_skip_events,
            first_boundary_skip_round: report.first_boundary_skip_round,
            last_boundary_skip_round: report.last_boundary_skip_round,
            max_rejoin_rounds: report.max_rejoin_rounds,
            longest_stale_rounds: report.longest_stale_rounds,
            unrejoined_nodes: report.unrejoined_nodes,
            horizon_skip_nodes: report.horizon_skip_nodes,
            wedged_round: report.wedged_round,
            failure: report.failure.clone(),
            recovery,
            pool_peak_in_flight: report.pool_peak_in_flight,
            spill_path: report.spill_path.clone(),
            commands,
            min_useful: report.metrics.iter().map(|m| m.useful).min(),
        }
    }

    /// The streamed construction: `retired` plus every tracker still held, and the row fold.
    pub(super) fn streamed(
        report_scalars: SmrReport,
        proto: Proto,
        retired: CommandFold,
        live: &[CommandTracker],
        rows: &RowFold,
    ) -> Self {
        debug_assert!(
            report_scalars.commands.is_empty() && report_scalars.metrics.is_empty(),
            "the streamed summary takes a report without per-command or per-round records"
        );
        let mut commands = retired;
        for t in live {
            commands.add(
                proto,
                &Landmarks::of_tracker(t),
                t.status(report_scalars.terminal),
            );
        }
        commands.paper.canonicalize();
        commands.client.canonicalize();
        let failed = report_scalars.terminal.is_failed();
        let recovery = report_scalars.recovery.map(|rec| RecoverySummary {
            fork_ok: rec.fork_ok,
            commit_fork_round: rec.commit_fork_round,
            rollbacks_total: rows.rollbacks_total,
            recovered_round: if failed {
                None
            } else {
                rows.recovered_candidate
            },
        });
        SmrSummary {
            terminal: report_scalars.terminal,
            safety_ok: report_scalars.safety_ok,
            exec_dup_round: report_scalars.exec_dup_round,
            retractions: report_scalars.retractions,
            retraction_rewrites: report_scalars.retraction_rewrites,
            first_retraction_round: report_scalars.first_retraction_round,
            strong_safety_violation_rounds: report_scalars.strong_safety_violation_rounds,
            strong_safety_first_violation: report_scalars.strong_safety_first_violation,
            strong_safety_boundary_violations: report_scalars.strong_safety_boundary_violations,
            strong_safety_boundary_first_violation: report_scalars
                .strong_safety_boundary_first_violation,
            repeat_skips: report_scalars.repeat_skips,
            first_repeat_skip_round: report_scalars.first_repeat_skip_round,
            boundary_skip_events: report_scalars.boundary_skip_events,
            first_boundary_skip_round: report_scalars.first_boundary_skip_round,
            last_boundary_skip_round: report_scalars.last_boundary_skip_round,
            max_rejoin_rounds: report_scalars.max_rejoin_rounds,
            longest_stale_rounds: report_scalars.longest_stale_rounds,
            unrejoined_nodes: report_scalars.unrejoined_nodes,
            horizon_skip_nodes: report_scalars.horizon_skip_nodes,
            wedged_round: report_scalars.wedged_round,
            failure: report_scalars.failure,
            recovery,
            pool_peak_in_flight: report_scalars.pool_peak_in_flight,
            spill_path: report_scalars.spill_path,
            commands,
            min_useful: rows.min_useful,
        }
    }
}
