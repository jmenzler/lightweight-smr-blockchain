//! Recovery-protocol (Algorithm 6) round step; the between-window pass runs after (f), RNG-free.

use crate::smr::engine::stages::{amplify, apply_all, failed_step, sn_order_failure};
use crate::smr::observe::canon_cursor::{CanonCursor, canon_lookup};
use crate::smr::observe::canonical::{
    CanonicalOrder, DigestCache, Executed, ForkSignature, timed_digest,
};
use crate::smr::observe::committed::{CommittedOracle, useful_at_round_end};
use crate::smr::observe::entry_index::EntryIndex;
use crate::smr::observe::exec_dup::ExecDupOracle;
use crate::smr::observe::log_index::{LogCounts, LogIndex, log_window_pos};
use crate::smr::observe::round_metrics::smr_round_metrics;
use crate::smr::tracker::{
    CommandTracker, advance_inert_prefix, latch_coverage, pos_stats, spreading_list,
};
use crate::smr::*;
use protocol::compact::{Entry, Timed, Triage};
use protocol::log::Log;
use protocol::recovery::{
    Checkpoint, PrefixMismatch, RState, RecoveryNode, RecoveryReply, latest_checkpoint_index,
};
use protocol::shared_state::ExecutedView;
use rand::Rng;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// Counted unconditionally: a gated counter's zero would read as "no volume" in a release binary.
static KEY_PROBES: AtomicU64 = AtomicU64::new(0);
static PRE_CALLS: AtomicU64 = AtomicU64::new(0);
static PRE_ENTRIES: AtomicU64 = AtomicU64::new(0);
static RECHECK_CALLS: AtomicU64 = AtomicU64::new(0);
static RECHECK_ENTRIES: AtomicU64 = AtomicU64::new(0);

/// (map probes, pre calls, pre entries, recheck calls, recheck entries) since process start.
pub fn boundary_key_counts() -> (u64, u64, u64, u64, u64) {
    (
        KEY_PROBES.load(Ordering::Relaxed),
        PRE_CALLS.load(Ordering::Relaxed),
        PRE_ENTRIES.load(Ordering::Relaxed),
        RECHECK_CALLS.load(Ordering::Relaxed),
        RECHECK_ENTRIES.load(Ordering::Relaxed),
    )
}

fn counted_pre(node: &RecoveryNode, round: u64, recheck: bool) -> Option<Vec<Timed>> {
    let pre = node.would_carry_pre(round);
    let entries = pre.as_ref().map_or(0, Vec::len) as u64;
    if recheck {
        RECHECK_CALLS.fetch_add(1, Ordering::Relaxed);
        RECHECK_ENTRIES.fetch_add(entries, Ordering::Relaxed);
    }
    PRE_CALLS.fetch_add(1, Ordering::Relaxed);
    PRE_ENTRIES.fetch_add(entries, Ordering::Relaxed);
    pre
}

#[cfg(test)]
pub(in crate::smr) struct RecBoundaryNodeSnapshot {
    checkpoint: Arc<Checkpoint>,
    log: Option<Log>,
    state: Arc<protocol::compact::SharedState>,
    r: RState,
}

pub(in crate::smr) struct RecRunState {
    t_window: u64,
    resend_until_acked: bool,
    prefix_mismatch: PrefixMismatch,
    pub(in crate::smr) rows: Vec<RecRoundMetrics>,
    /// False on a checkpoint-lineage difference (Lemma 6.5), not on a fork of the committed
    /// sequences, which `safety_ok` reports.
    pub(in crate::smr) fork_ok: bool,
    pub(in crate::smr) commit_fork_round: Option<usize>,
    pending_rollbacks: u32,
    pending_rollback_depth: u32,
    pending_cp_fork_k: u32,
    pending_max_cp_p_len: u32,
    prev_len: Vec<u64>,
    canonical_pos: EntryIndex,
    lineage: BTreeMap<u64, ForkSignature>,
    /// Per W, the distinct checkpoints some server committed at a window end.
    committed_signatures: BTreeMap<u64, Vec<ForkSignature>>,
    last_window: Vec<Option<u64>>,
    /// Lowest checkpoint window `lineage` and `committed_signatures` still answer for.
    lineage_floor: u64,
}

impl RecRunState {
    pub(in crate::smr) fn new(
        t_window: u64,
        n: usize,
        resend_until_acked: bool,
        prefix_mismatch: PrefixMismatch,
    ) -> Self {
        RecRunState {
            t_window,
            resend_until_acked,
            prefix_mismatch,
            rows: Vec::new(),
            fork_ok: true,
            commit_fork_round: None,
            pending_rollbacks: 0,
            pending_rollback_depth: 0,
            pending_cp_fork_k: 0,
            pending_max_cp_p_len: 0,
            prev_len: vec![0; n],
            canonical_pos: EntryIndex::new(),
            lineage: BTreeMap::new(),
            committed_signatures: BTreeMap::new(),
            last_window: vec![None; n],
            lineage_floor: 0,
        }
    }

    pub(in crate::smr) fn t_window(&self) -> u64 {
        self.t_window
    }

    pub(in crate::smr) fn resend_until_acked(&self) -> bool {
        self.resend_until_acked
    }

    /// Drops lineage below `lowest`, every node's current checkpoint window. Windows never
    /// regress per node, so later lookups ask for a window at or above it: an exact key there,
    /// or the nearest key below one, which is at worst the largest key below `lowest`, kept.
    pub(in crate::smr) fn prune_lineage_below(&mut self, lowest: u64) {
        if lowest <= self.lineage_floor {
            return;
        }
        let keep_from = self
            .lineage
            .range(..lowest)
            .next_back()
            .map_or(lowest, |(&w, _)| w);
        self.lineage = self.lineage.split_off(&keep_from);
        self.committed_signatures = self.committed_signatures.split_off(&lowest);
        self.lineage_floor = lowest;
    }

    fn assert_above_lineage_floor(&self, w: u64) {
        assert!(
            w >= self.lineage_floor,
            "checkpoint window {w} is below the pruned lineage floor {}",
            self.lineage_floor
        );
    }

    pub(in crate::smr) fn heap_bytes_rows(&self) -> u64 {
        (self.rows.capacity() * size_of::<RecRoundMetrics>()
            + self.prev_len.capacity() * size_of::<u64>()
            + self.last_window.capacity() * size_of::<Option<u64>>()) as u64
    }

    #[cfg(test)]
    pub(in crate::smr) fn canonical_pos_resident(&self) -> usize {
        self.canonical_pos.resident()
    }

    pub(in crate::smr) fn heap_bytes_indexes(&self) -> u64 {
        self.canonical_pos.heap_bytes()
    }

    pub(in crate::smr) fn heap_bytes_lineage(&self) -> u64 {
        let committed: usize = self
            .committed_signatures
            .values()
            .map(|v| size_of::<u64>() + v.capacity() * size_of::<ForkSignature>())
            .sum();
        (((self.lineage.len() * (size_of::<u64>() + size_of::<ForkSignature>()) + committed)
            as u64)
            * 3)
            / 2
    }

    /// Lemma 6.9: a server's executed length never shrinks; a shrink is returned to be recorded.
    fn check_growth(&mut self, node: usize, executed_len: u64) -> Result<(), RecoveryViolation> {
        let previous_len = self.prev_len[node];
        self.prev_len[node] = executed_len;
        if executed_len < previous_len {
            return Err(RecoveryViolation::ExecutedRegression {
                previous_len,
                len: executed_len,
            });
        }
        Ok(())
    }

    /// Growth and lineage over every node; the first shrink, to be recorded. Runs before the
    /// double-execution oracle, whose per-server slots assume no server shrank.
    fn check_nodes<'a>(
        &mut self,
        nodes: &'a [RecoveryNode],
        digests: &mut DigestCache<'a>,
    ) -> Option<(usize, RecoveryViolation)> {
        let mut regression = None;
        for (i, nd) in nodes.iter().enumerate() {
            if let Err(violation) = self.check_growth(i, nd.shared_state().logical_len()) {
                regression.get_or_insert((i, violation));
            }
            self.check_lineage(i, nd.checkpoint(), digests);
        }
        regression
    }

    fn check_lineage<'a>(
        &mut self,
        node: usize,
        checkpoint: &'a Checkpoint,
        digests: &mut DigestCache<'a>,
    ) {
        // A checkpoint's window strictly rises on any change, so an unchanged window is the same checkpoint.
        let last = self.last_window[node];
        assert!(
            last.is_none_or(|w| w <= checkpoint.w),
            "node {node}: checkpoint window regressed"
        );
        if last == Some(checkpoint.w) {
            return;
        }
        self.last_window[node] = Some(checkpoint.w);
        self.assert_above_lineage_floor(checkpoint.w);
        let signature = digests.signature(checkpoint);
        match self.lineage.get(&checkpoint.w) {
            Some(mark) => {
                if *mark != signature {
                    self.fork_ok = false;
                }
            }
            None => {
                let broken =
                    self.lineage
                        .range(..checkpoint.w)
                        .next_back()
                        .is_some_and(|(_, prev)| {
                            !signature.extends(
                                digests.canon(),
                                checkpoint,
                                prev.executed_len(),
                                prev.executed(),
                            )
                        });
                if broken {
                    self.fork_ok = false;
                }
                self.lineage.insert(checkpoint.w, signature);
            }
        }
    }

    /// Lemma 6.7 on the commit path: `checkpoint` is committed at this window end. Dates the
    /// first window end at which its W holds a second distinct committed checkpoint.
    fn record_commit<'a>(
        &mut self,
        checkpoint: &'a Checkpoint,
        digests: &mut DigestCache<'a>,
        round: usize,
    ) {
        self.assert_above_lineage_floor(checkpoint.w);
        let signature = digests.signature(checkpoint);
        let seen = self.committed_signatures.entry(checkpoint.w).or_default();
        if !seen.contains(&signature) {
            seen.push(signature);
            if seen.len() == 2 {
                self.commit_fork_round.get_or_insert(round);
            }
        }
    }
}

/// p. 6, commands are unique: a double execution is one server's executed sequence holding a
/// (client, sn) twice. `replaced[i]` marks a server whose S_i may have been swapped this step.
fn observe_exec_dup(
    exec_dup: &mut ExecDupOracle,
    prefix_ok: bool,
    canonical: &CanonicalOrder,
    nodes: &[RecoveryNode],
    replaced: &[bool],
    round: usize,
) {
    if prefix_ok {
        exec_dup.observe_canonical(canonical.start(), canonical.retained(), round);
    } else if exec_dup.tripped_round().is_none() {
        let executed: Vec<ExecutedView<'_>> = nodes
            .iter()
            .map(|nd| nd.shared_state().executed())
            .collect();
        exec_dup.observe_nodes(&executed, replaced, round);
    }
}

fn observe_committed(
    committed: &mut CommittedOracle,
    canonical: &CanonicalOrder,
    nodes: &[RecoveryNode],
    round: usize,
) {
    let executed: Vec<ExecutedView<'_>> = nodes
        .iter()
        .map(|nd| nd.shared_state().executed())
        .collect();
    committed.observe(&executed, canonical, round);
}

/// Whether the between-window steps would commit C_i's P on this node: it does not skip the
/// boundary, P ≠ ⊥, and it rolls back (R_i = reset, L_i := P) or holds a log P prefixes.
fn commits_at_window_end(nd: &RecoveryNode) -> bool {
    !nd.stale_checkpoint()
        && nd.checkpoint().p.is_some()
        && (nd.reset_state() == RState::Reset || nd.log_seq().is_some())
        && nd.validate_boundary().is_ok()
}

/// A shrunk executed sequence (Lemma 6.9) ends the run as a recorded failure in this round.
fn regression_failure(node: usize, violation: RecoveryViolation, round: usize) -> RunFailure {
    RunFailure {
        node: node as u32,
        attempted_round: round,
        completed_round: round - 1,
        observed_round: round,
        phase: RunFailurePhase::ExecutedRegression,
        violation,
    }
}

/// The report fields are u32; a count past that must stop the run rather than wrap.
fn report_u32(value: u64, what: &str) -> u32 {
    u32::try_from(value).unwrap_or_else(|_| panic!("{what} {value} exceeds the u32 report field"))
}

/// The checkpoints at the largest W after a boundary, classed twice.
struct NewestClasses {
    /// Distinct by fork signature, P read as commands (Lemma 6.5/6.7).
    fork_k: u32,
    max_pre_len: u32,
    /// Interning classes, P read with its stamps: an adopted P' becomes L_i, stamps and all,
    /// and the stamps drive aging. `member_of[i]` names node i's class unless it represents one.
    representatives: Vec<Arc<Checkpoint>>,
    member_of: Vec<Option<usize>>,
}

// Keyed by digests, not `Checkpoint::eq`, whose equality asserts equal offsets.
fn newest_window_classes<'a>(
    nodes: &'a [RecoveryNode],
    digests: &mut DigestCache<'a>,
) -> NewestClasses {
    let max_w = nodes.iter().map(|nd| nd.checkpoint().w).max().unwrap_or(0);
    let mut distinct: Vec<ForkSignature> = Vec::new();
    let mut intern_keys: Vec<(ForkSignature, Option<u64>)> = Vec::new();
    let mut representatives: Vec<Arc<Checkpoint>> = Vec::new();
    let mut member_of: Vec<Option<usize>> = vec![None; nodes.len()];
    for (i, nd) in nodes.iter().enumerate() {
        let cp = nd.checkpoint();
        if cp.w != max_w {
            continue;
        }
        let signature = digests.signature(cp);
        if !distinct.contains(&signature) {
            distinct.push(signature);
        }
        let key = (signature, cp.p.as_deref().map(timed_digest));
        match intern_keys.iter().position(|k| *k == key) {
            Some(class) => member_of[i] = Some(class),
            None => {
                intern_keys.push(key);
                representatives.push(nd.checkpoint_shared());
            }
        }
    }
    NewestClasses {
        fork_k: distinct.len() as u32,
        max_pre_len: report_u32(
            distinct
                .iter()
                .filter_map(ForkSignature::pre_len)
                .max()
                .unwrap_or(0) as u64,
            "checkpoint prefix length",
        ),
        representatives,
        member_of,
    }
}

pub(in crate::smr) fn r_code(r: RState) -> u8 {
    match r {
        RState::NoReset => 0,
        RState::Reset => 1,
        RState::Bot => 2,
    }
}

impl SmrState {
    pub(in crate::smr) fn step_rec(&mut self, mask: &[bool]) -> SmrStepStatus {
        let n = self.n;
        let cfg = self.cfg;
        let amp = self.amp;
        let round = self.round;
        let arrivals = std::mem::take(&mut self.arrivals_pending);
        let SmrNodes::Rec(nodes) = &mut self.nodes else {
            unreachable!("dispatched on Rec");
        };
        // The node swaps, never edits, its log, so the handle IS the pre-step snapshot.
        let snap_logs: Vec<Option<Log>> =
            nodes.iter().map(|nd| nd.log_perm_arc().cloned()).collect();
        let snap_cp_windows: Vec<u64> = nodes.iter().map(|nd| nd.checkpoint().w).collect();
        #[cfg(debug_assertions)]
        let snap_cps: Vec<_> = nodes.iter().map(|nd| nd.checkpoint().clone()).collect();
        let snap_rs: Vec<RState> = nodes.iter().map(RecoveryNode::reset_state).collect();
        // p. 19 useful at the round start: the row's `useful` count and the census read it, the Def 1.4
        // counters read `useful_at_round_end`.
        let useful_at_start: Vec<bool> =
            (0..n).map(|i| snap_logs[i].is_some() && !mask[i]).collect();
        let useful_total = useful_at_start.iter().filter(|&&u| u).count() as u32;

        // (b) client stage: servers triage by Alg 5's sn rule; resend until acked, or until §6-committed everywhere
        let ack_mode = self.rec.as_ref().is_some_and(|r| r.resend_until_acked);
        let mut delivered: Vec<(usize, ClientCommand)> = Vec::new();
        for &ti in &self.client_order {
            let t = &mut self.trackers[ti];
            let pending = if ack_mode {
                t.committed_ack_round.is_none()
            } else {
                t.executed_round.is_none()
            };
            if pending && round >= t.injection_round {
                let target = match t.target {
                    Some(id) => id as usize,
                    None => self.rng.random_range(0..n),
                };
                #[cfg(debug_assertions)]
                assert_eq!(
                    nodes[target].checkpoint(),
                    &snap_cps[target],
                    "stage (b) read post-step state"
                );
                if !mask[target] {
                    t.injected_round.get_or_insert(round);
                }
                let outcome = if mask[target] {
                    AttemptOutcome::TargetBlocked
                } else if snap_logs[target].is_none() {
                    AttemptOutcome::TargetBot
                } else {
                    match nodes[target].on_client_command(&t.cc) {
                        Triage::AckCommitted => {
                            // The ack client is done here (p. 25); its slot is drawable next round.
                            if ack_mode && let Some(pool) = &mut self.pool {
                                pool.free(t.client);
                            }
                            t.committed_ack_round.get_or_insert(round);
                            AttemptOutcome::AckCommitted
                        }
                        Triage::Amplify => {
                            if t.delivered_round.is_none() {
                                t.delivered_round = Some(round);
                            }
                            delivered.push((target, t.cc));
                            AttemptOutcome::Delivered
                        }
                        Triage::Ignore => AttemptOutcome::Ignored,
                    }
                };
                t.attempts.push(DeliveryAttempt {
                    round,
                    target: target as u32,
                    outcome,
                });
            }
        }

        // (c) amplify stage: a resend re-amplifies only if the target's log lost the command
        let partition = self.partition.as_deref();
        let appends = amplify(
            &mut self.rng,
            &mut self.trackers,
            delivered,
            |server, cc| nodes[server].wants_amplify(cc),
            mask,
            partition,
            n,
            amp,
            round,
        );

        // (d) request stage, answered by `answer`: log-repliers ⊆ recovery-repliers (Lemma 6.2)
        let mut inboxes: Vec<Vec<RecoveryReply>> = Vec::with_capacity(n);
        let mut sources: Vec<Vec<u32>> = Vec::with_capacity(n);
        for i in 0..n {
            let mut replies = Vec::with_capacity(cfg.k);
            let mut senders = Vec::with_capacity(cfg.k);
            for _ in 0..cfg.k {
                let target = self.rng.random_range(0..n);
                if !mask[i]
                    && !mask[target]
                    && same_component(partition, i, target)
                    && let Some(reply) = nodes[target].answer()
                {
                    #[cfg(debug_assertions)]
                    assert_eq!(
                        nodes[target].checkpoint(),
                        &snap_cps[target],
                        "stage (d) read post-step state"
                    );
                    replies.push(reply);
                    senders.push(target as u32);
                }
            }
            inboxes.push(replies);
            sources.push(senders);
        }

        // (e) core step: all draws first, ascending id, reading only stage-(d) inboxes; applies are RNG-free.
        let choices: Vec<Option<Vec<usize>>> = (0..n)
            .map(|i| nodes[i].draw_reply_choice(&inboxes[i], &mut self.rng))
            .collect();
        apply_all(nodes, |i, node: &mut RecoveryNode| {
            node.step_chosen(&inboxes[i], &appends[i], choices[i].as_deref());
        });

        // (f) observation: no RNG from here on
        let mut adoptions: Vec<(u32, u32)> = Vec::new();
        let mut reset_votes: Vec<(u32, u32)> = Vec::new();
        for (i, replies) in inboxes.iter().enumerate() {
            if replies.len() < cfg.ell {
                continue;
            }
            if nodes[i].checkpoint().w > snap_cp_windows[i] {
                adoptions.push((sources[i][latest_checkpoint_index(replies)], i as u32));
            }
            if snap_rs[i] != RState::NoReset
                && nodes[i].reset_state() == RState::NoReset
                && let Some(j) = replies.iter().position(|rep| rep.r_j == RState::NoReset)
            {
                reset_votes.push((sources[i][j], i as u32));
            }
        }
        drop(inboxes);

        let rec = self.rec.as_mut().expect("recovery run state");
        let post_logs: Vec<Option<&protocol::chunked::ChunkSeq<Timed>>> =
            nodes.iter().map(RecoveryNode::log_seq).collect();
        let (post_index, distinct_logs) =
            LogIndex::build_with_distinct(post_logs.iter().copied(), self.observe_spread);
        let executed_lens: Vec<usize> = nodes
            .iter()
            .map(|nd| nd.shared_state().logical_len() as usize)
            .collect();
        let round_metrics = smr_round_metrics(
            &post_logs,
            &executed_lens,
            mask,
            useful_total,
            distinct_logs,
            arrivals,
        );
        self.metrics.push(round_metrics);

        // Read before this round's check extends it.
        let pos_floor = report_u32(self.checker.canonical_len() as u64, "canonical length");

        if self.safety_ok {
            let executed: Vec<ExecutedView<'_>> = nodes
                .iter()
                .map(|nd| nd.shared_state().executed())
                .collect();
            if !self.checker.check_round(&executed, self.frontier) {
                self.safety_ok = false;
            }
        }
        let cursors_exact = self.safety_ok;
        let canonical = self.checker.canonical();
        // Pre-mint checkpoints: this round's adoptions here, the committing ones at the boundary.
        let mut digests = DigestCache::new(canonical);
        let regression = rec.check_nodes(nodes, &mut digests);
        // Alg 6 swaps S_i for a peer's only on adopting a newer checkpoint.
        let adopted: Vec<bool> = nodes
            .iter()
            .zip(&snap_cp_windows)
            .map(|(nd, &w)| nd.checkpoint().w > w)
            .collect();
        if regression.is_none() {
            observe_exec_dup(
                &mut self.exec_dup,
                self.safety_ok,
                canonical,
                nodes,
                &adopted,
                round,
            );
        }
        observe_committed(&mut self.committed, canonical, nodes, round);
        rec.canonical_pos
            .extend_first(canonical.start(), canonical.retained());
        let canon_index = &rec.canonical_pos;

        let observe_spread = self.observe_spread;
        let useful_counts = observe_spread.then(|| {
            LogCounts::build(snap_logs.iter().zip(&useful_at_start).map(|(log, &u)| {
                if u {
                    log.as_ref().map(|l| &**l.entries())
                } else {
                    None
                }
            }))
        });

        let drop_settled_spread = self.drop_settled_spread;
        let inert = |t: &CommandTracker| {
            t.executed_round.is_some_and(|r| round > r + 1)
                && t.all_logs_round.is_some()
                && t.canon_cursor.is_none()
                && (!drop_settled_spread || t.spread_points == 0)
                && (!ack_mode || t.committed_ack_round.is_some())
        };
        self.inert_prefix = advance_inert_prefix(&self.trackers, self.inert_prefix, inert);
        let live_from = self.inert_prefix;
        let mut live_floor: Option<u32> = None;
        #[cfg(debug_assertions)]
        assert!(
            self.trackers[..live_from].iter().all(inert),
            "round {round}: skipped a tracker that is not inert"
        );
        for t in self.trackers[live_from..].iter_mut() {
            if round < t.injection_round {
                continue;
            }
            let entry = Entry::Cmd(t.cc);
            if t.executed_round.is_none_or(|r| round <= r + 1) {
                t.spread_points += 1;
                crate::smr::observe::census_count::window();
                let cursor = t
                    .canon_cursor
                    .get_or_insert_with(|| CanonCursor::new(pos_floor));
                live_floor = Some(live_floor.unwrap_or(u32::MAX).min(cursor.floor()));
                let cursor_floor = u64::from(cursor.floor());
                if observe_spread {
                    let useful_holders = useful_counts
                        .as_ref()
                        .expect("census on implies the counts were built")
                        .count(&entry);
                    let entry_holders = post_index.entry_holders(&entry);
                    let mut visits = 0usize;
                    let hit = cursors_exact
                        .then(|| canon_index.get_from(&entry, cursor_floor))
                        .flatten();
                    let (pos_min, pos_med, pos_max) = pos_stats(
                        nodes
                            .iter()
                            .enumerate()
                            .filter(|(_, nd)| nd.log_seq().is_some())
                            .inspect(|_| visits += 1)
                            .filter_map(|(i, nd)| {
                                let ex = nd.shared_state().executed();
                                let executed = Executed {
                                    view: ex,
                                    canonical,
                                };
                                (if cursors_exact {
                                    let found = canon_lookup(hit, ex);
                                    debug_assert_eq!(
                                        found,
                                        executed.front_scan(&entry),
                                        "node {i}: canonical position differs \
                                         from the front scan"
                                    );
                                    found
                                } else {
                                    executed.front_scan(&entry)
                                })
                                .or_else(|| {
                                    log_window_pos(
                                        entry_holders,
                                        i as u32,
                                        nd.log_seq().expect("filtered non-⊥"),
                                        &entry,
                                        ex.logical_len(),
                                    )
                                })
                            })
                            .collect(),
                    );
                    crate::smr::observe::census_count::point(visits);
                    t.spread.push(SpreadPoint {
                        round,
                        useful_holders,
                        useful_total,
                        pos_min,
                        pos_med,
                        pos_max,
                    });
                    debug_assert_eq!(
                        t.spread_points as usize,
                        t.spread.len(),
                        "op {}: the point counter drifted from the curve",
                        t.op
                    );
                }
            } else {
                t.canon_cursor = None;
                if drop_settled_spread {
                    t.shed_spread();
                    if !ack_mode || t.committed_ack_round.is_some() {
                        t.shed_attempts();
                    }
                }
            }
            latch_coverage(t, &entry, round, &post_index, &post_logs);
        }
        drop(useful_counts);
        drop(snap_logs);

        let spreading = if drop_settled_spread {
            Vec::new()
        } else {
            spreading_list(
                &self.trackers,
                live_from,
                round,
                &post_index,
                &post_logs,
                |t| t.executed_round.is_none(),
            )
        };
        drop(post_index);

        let count_r = |want: RState| nodes.iter().filter(|nd| nd.reset_state() == want).count();
        rec.rows.push(RecRoundMetrics {
            noreset: count_r(RState::NoReset) as u32,
            reset: count_r(RState::Reset) as u32,
            bot_r: count_r(RState::Bot) as u32,
            window: (round as u64 - 1) / rec.t_window,
            rollbacks: std::mem::take(&mut rec.pending_rollbacks),
            max_checkpoint_window: nodes.iter().map(|nd| nd.checkpoint().w).max().unwrap_or(0),
            rollback_depth: std::mem::take(&mut rec.pending_rollback_depth),
            cp_fork_k: std::mem::take(&mut rec.pending_cp_fork_k),
            max_cp_p_len: std::mem::take(&mut rec.pending_max_cp_p_len),
            useful_agree: None,
        });

        for (i, nd) in nodes.iter().enumerate() {
            debug_assert!(
                nd.reset_state() != RState::Bot || nd.log_seq().is_none(),
                "node {i}: Lemma 6.2 violated in round {round}"
            );
        }

        let glances = post_logs
            .iter()
            .zip(&executed_lens)
            .zip(nodes.iter())
            .map(|((log, &ex), nd)| NodeGlance {
                log_len: log.map(|l| report_u32(l.len() as u64, "log length")),
                executed_len: report_u32(ex as u64, "executed length"),
                r: Some(r_code(nd.reset_state())),
            })
            .collect();
        drop(post_logs);
        let rec_info = RecStepInfo {
            adoptions,
            reset_votes,
        };

        if let Some((node, violation)) = regression {
            let failure = regression_failure(node, violation, round);
            let status = failed_step(
                &failure,
                round_metrics,
                mask,
                glances,
                spreading,
                Some(rec_info),
            );
            self.failure = Some(failure);
            self.failed_status = Some(status.clone());
            return status;
        }

        // Between-window pass: RNG-free; all-⊥ is expected mid-surge, so recovery never reads Dead.
        #[cfg(test)]
        if (round as u64).is_multiple_of(rec.t_window) {
            self.rec_boundary_snapshot = Some(
                nodes
                    .iter()
                    .map(|node| RecBoundaryNodeSnapshot {
                        checkpoint: node.checkpoint_shared(),
                        log: node.log_perm_arc().cloned(),
                        state: Arc::clone(node.state_arc()),
                        r: node.reset_state(),
                    })
                    .collect(),
            );
        }
        let boundary = (round as u64).is_multiple_of(rec.t_window);
        // Lemma 6.7 on the commit path, which the node-local check below cannot see for a
        // rollback: dated only, so Abort runs keep the paper's stopping rule.
        if boundary {
            for nd in nodes.iter().filter(|nd| commits_at_window_end(nd)) {
                rec.record_commit(nd.checkpoint(), &mut digests, round);
            }
        }
        let abort = (boundary && rec.prefix_mismatch == PrefixMismatch::Abort)
            .then(|| {
                nodes.iter().enumerate().find_map(|(node, nd)| {
                    nd.validate_boundary()
                        .err()
                        .map(|v| (node, RecoveryViolation::Prefix(v)))
                })
            })
            .flatten();
        if let Some((node, violation)) = abort {
            let failure = RunFailure {
                node: node as u32,
                attempted_round: round,
                completed_round: round - 1,
                observed_round: round,
                phase: RunFailurePhase::BoundaryPreflight,
                violation,
            };
            let status = failed_step(
                &failure,
                round_metrics,
                mask,
                glances,
                spreading,
                Some(rec_info),
            );
            self.failure = Some(failure);
            self.failed_status = Some(status.clone());
            return status;
        }
        #[cfg(test)]
        if (round as u64).is_multiple_of(rec.t_window) {
            // Drop test-only Arc handles so the boundary's copy-on-write does not clone.
            self.rec_boundary_snapshot = None;
        }
        if (round as u64).is_multiple_of(rec.t_window) {
            let next_window = round as u64 / rec.t_window;
            // A rollback swaps S_i for C_i's S (Alg 6 "If R_i = reset").
            let rolled_back: Vec<bool> = nodes
                .iter()
                .map(|nd| nd.reset_state() == RState::Reset)
                .collect();
            // Read before `end_window` re-mints the checkpoint window.
            // A stale node skips the boundary (DEV-3) and does not roll back, whatever its R_i.
            let rolling = nodes
                .iter()
                .filter(|nd| nd.reset_state() == RState::Reset && !nd.stale_checkpoint())
                .map(|nd| nd.checkpoint().w);
            rec.pending_rollbacks = 0;
            rec.pending_rollback_depth = 0;
            for cp_window in rolling {
                rec.pending_rollbacks += 1;
                rec.pending_rollback_depth = rec
                    .pending_rollback_depth
                    .max(report_u32(next_window - cp_window, "rollback depth"));
            }
            // Per-class mint keyed sound-over-complete: a missed equality only costs memory, never correctness.
            let mut keyed: HashMap<(usize, u64, usize), (usize, Vec<Timed>)> = HashMap::new();
            let mut mint_for: Vec<Option<usize>> = vec![None; nodes.len()];
            for i in 0..nodes.len() {
                let Some(pre) =
                    counted_pre(&nodes[i], round as u64, false).filter(|_| self.safety_ok)
                else {
                    continue;
                };
                let key = (
                    std::ptr::from_ref(nodes[i].checkpoint()) as usize,
                    timed_digest(&pre),
                    pre.len(),
                );
                KEY_PROBES.fetch_add(1, Ordering::Relaxed);
                match keyed.get(&key) {
                    // The digest is a prefilter; the kept representative prefix is the proof.
                    Some((j, rep)) => {
                        let j = *j;
                        let matches = *rep == pre;
                        #[cfg(debug_assertions)]
                        {
                            let fresh = counted_pre(&nodes[j], round as u64, true);
                            assert_eq!(
                                fresh.as_ref(),
                                Some(rep),
                                "the kept representative prefix diverged from a fresh derivation"
                            );
                        }
                        if matches {
                            mint_for[i] = Some(j);
                        }
                    }
                    None => {
                        keyed.insert(key, (i, pre));
                    }
                }
            }
            let mut minted: Vec<Option<Arc<Checkpoint>>> = vec![None; nodes.len()];
            for i in 0..nodes.len() {
                #[cfg(debug_assertions)]
                {
                    let nd = &nodes[i];
                    let (live, held) = (nd.shared_state(), &nd.checkpoint().s);
                    debug_assert!(
                        live.logical_len() == held.logical_len()
                            && live.sn_iter().eq(held.sn_iter()),
                        "round {round}: node {i} entered the boundary with a live state that \
                         had left its checkpoint behind — the mint-dedup key relies on the two \
                         agreeing, sequence numbers included"
                    );
                }
                match mint_for[i] {
                    Some(j) => {
                        let cp = minted[j].clone().expect("a class mints before its members");
                        nodes[i].end_window_shared(next_window, round as u64, &cp);
                    }
                    None => {
                        nodes[i].end_window(next_window, round as u64);
                        minted[i] = Some(nodes[i].checkpoint_shared());
                    }
                }
                // A refusing representative minted nothing its class members could hold.
                if let Some(gap) = nodes[i].sn_order_violation() {
                    let failure = sn_order_failure(i, gap, round);
                    let status = failed_step(
                        &failure,
                        round_metrics,
                        mask,
                        glances,
                        spreading,
                        Some(rec_info),
                    );
                    self.failure = Some(failure);
                    self.failed_status = Some(status.clone());
                    return status;
                }
            }

            // p. 33: a window ends before the between-window steps, so this round's verdicts
            // cover what the boundary committed; anything reading positions comes after.
            if self.safety_ok {
                let executed: Vec<ExecutedView<'_>> = nodes
                    .iter()
                    .map(|nd| nd.shared_state().executed())
                    .collect();
                if !self.checker.check_round(&executed, self.frontier) {
                    self.safety_ok = false;
                }
            }
            let longest = nodes
                .iter()
                .map(|nd| nd.shared_state().executed())
                .max_by_key(ExecutedView::logical_len)
                .expect("at least one node");
            self.checker.absorb(longest);
            let canonical = self.checker.canonical();
            let mut digests = DigestCache::new(canonical);
            // check_round compares only what grew since a node's last check; the digest covers
            // its whole retained suffix, which the lean pass is about to forget.
            if self.safety_ok
                && !nodes.iter().all(|nd| {
                    let st = nd.shared_state();
                    digests.state(st) == canonical.digest_at(st.logical_len())
                })
            {
                self.safety_ok = false;
            }
            let verified = canonical.len();
            let regression = rec.check_nodes(nodes, &mut digests);
            if regression.is_none() {
                observe_exec_dup(
                    &mut self.exec_dup,
                    self.safety_ok,
                    canonical,
                    nodes,
                    &rolled_back,
                    round,
                );
            }
            observe_committed(&mut self.committed, canonical, nodes, round);
            if let Some((node, violation)) = regression {
                let failure = regression_failure(node, violation, round);
                let status = failed_step(
                    &failure,
                    round_metrics,
                    mask,
                    glances,
                    spreading,
                    Some(rec_info),
                );
                self.failure = Some(failure);
                self.failed_status = Some(status.clone());
                return status;
            }

            let classes = newest_window_classes(nodes, &mut digests);
            rec.pending_cp_fork_k = classes.fork_k;
            rec.pending_max_cp_p_len = classes.max_pre_len;

            // p. 36: a command is committed once it leaves P between windows and every useful
            // server has executed it, so the landmark is dated at a boundary.
            let pending = self.trackers[live_from..]
                .iter()
                .any(|t| t.executed_round.is_none());
            let release_line = self.release_line;
            let executed_pos = if self.safety_ok && pending {
                rec.canonical_pos
                    .extend_first(canonical.start(), canonical.retained());
                Some(&rec.canonical_pos)
            } else {
                None
            };
            for t in self.trackers[live_from..].iter_mut() {
                if t.executed_round.is_some() {
                    continue;
                }
                let entry = Entry::Cmd(t.cc);
                let mut any_useful = false;
                let all_executed = match executed_pos {
                    Some(pos) => {
                        // An open cursor's floor bounds the release line, so it is a valid floor.
                        let floor = t
                            .canon_cursor
                            .as_ref()
                            .map_or(release_line, |c| u64::from(c.floor()));
                        let at = pos.get_from(&entry, floor);
                        nodes.iter().all(|nd| {
                            if nd.log_seq().is_none() {
                                return true;
                            }
                            any_useful = true;
                            at.is_some_and(|p| u64::from(p) < nd.shared_state().logical_len())
                        })
                    }
                    None => nodes.iter().all(|nd| {
                        if nd.log_seq().is_none() {
                            return true;
                        }
                        any_useful = true;
                        Executed {
                            view: nd.shared_state().executed(),
                            canonical,
                        }
                        .front_scan(&entry)
                        .is_some()
                    }),
                };
                if any_useful && all_executed {
                    t.executed_round = Some(round);
                    // The non-ack client stops on this latch, so its slot frees with it.
                    if !ack_mode && let Some(pool) = &mut self.pool {
                        pool.free(t.client);
                    }
                }
            }

            // (g) release and forget: RNG-free and deliberately last.
            if self.truncate_history
                && !self.safety_ok
                && let Some(spill) = &mut self.spill
            {
                spill.mark_diverged();
            }
            if self.truncate_history && self.safety_ok {
                let entering = self.release_line;
                if let Some(i) = nodes
                    .iter()
                    .position(|nd| nd.shared_state().logical_len() < entering)
                {
                    let report = crate::smr::observe::spill::diagnose(
                        self.spill
                            .as_ref()
                            .map(crate::smr::observe::spill::SpillWriter::path),
                        i,
                        round,
                        nodes[i].shared_state().executed(),
                    );
                    panic!(
                        "driver bug: node {i} holds {} executed commands at round \
                         {round}, below the observer release line {entering}. The \
                         release line is the minimum executed length over the nodes, and \
                         the growth check above ends the run as a recorded \
                         ExecutedRegression the moment any node's length shrinks, so that \
                         minimum is monotone and no restored state can dip below it. A \
                         firing therefore means the OBSERVER released a position a node \
                         can still re-present. It does NOT mean the protocol misbehaved.\n{report}",
                        nodes[i].shared_state().logical_len()
                    );
                }

                let checkpoint_lens: Vec<u64> = nodes
                    .iter()
                    .map(|nd| nd.checkpoint().s.logical_len())
                    .collect();
                let rollback_floor = checkpoint_lens
                    .iter()
                    .copied()
                    .min()
                    .expect("at least one node");
                let restorable_floor = nodes
                    .iter()
                    .map(|nd| nd.checkpoint().s.executed_offset())
                    .min()
                    .expect("at least one node");
                let cursor_floor = live_floor.map_or(verified, |f| u64::from(f).min(verified));
                let release = entering.max(rollback_floor.min(cursor_floor).min(restorable_floor));
                debug_assert!(
                    release <= restorable_floor,
                    "round {round}: the release line {release} passed the deepest \
                     restorable offset {restorable_floor}"
                );

                // Spill before anything forgets: afterwards it is the only record of the committed stream.
                if self.spill.is_none() && self.spill_allowed {
                    let (_, commit, dirty) = crate::runlog::provenance();
                    let created = crate::smr::observe::spill::SpillWriter::create(
                        crate::smr::observe::spill::SpillHeader {
                            commit,
                            dirty,
                            seed: self.seed,
                            n,
                            t_window: rec.t_window,
                            proto: "recovery".into(),
                        },
                        self.scenario_tag.as_deref(),
                    );
                    self.spill = Some(created.unwrap_or_else(|cause| {
                        spill_io_failure("create", self.seed, None, &cause)
                    }));
                }
                if let Some(spill) = &mut self.spill {
                    let written = spill.boundary(
                        canonical,
                        next_window,
                        round,
                        checkpoint_lens,
                        nodes
                            .iter()
                            .map(|nd| nd.shared_state().logical_len())
                            .collect(),
                        release,
                    );
                    if let Err(cause) = written {
                        spill_io_failure("boundary write", self.seed, Some(spill.path()), &cause);
                    }
                }

                self.release_line = release;
                self.checker.release_to(release);
                rec.canonical_pos.release_to(release);

                let mut frontier = self.frontier;
                for node in nodes.iter_mut() {
                    node.forget_committed_prefix(node.checkpoint().s.logical_len());
                    frontier = frontier.max(node.shared_state().executed_offset());
                }
                self.frontier = frontier;

                #[cfg(debug_assertions)]
                {
                    for (i, nd) in nodes.iter().enumerate() {
                        assert!(
                            nd.shared_state().executed_offset() >= release,
                            "round {round}: node {i} forgot to {} but the observer \
                             released to {release}, so nothing retains what it dropped",
                            nd.shared_state().executed_offset()
                        );
                    }
                    if let Some(floor) = live_floor {
                        assert!(
                            release <= u64::from(floor),
                            "round {round}: the release line {release} passed a live \
                             cursor floor {floor}"
                        );
                    }
                }
            }

            // Interning runs last and only while the safety latch holds.
            if self.safety_ok {
                for (i, node) in nodes.iter_mut().enumerate() {
                    if let Some(class) = classes.member_of[i] {
                        node.share_checkpoint(&classes.representatives[class]);
                    }
                }
            }
        }

        let useful_end = useful_at_round_end(nodes.iter().map(|nd| nd.log_seq().is_some()), mask);
        let agreement = self.committed.check_strong_safety(
            &useful_end,
            round,
            (round as u64).is_multiple_of(rec.t_window),
        );
        rec.rows
            .last_mut()
            .expect("this round's row is pushed")
            .useful_agree = agreement;

        SmrStepStatus {
            round,
            metrics: round_metrics,
            blocked: blocked_ids(mask),
            nodes: glances,
            spreading,
            dead: None,
            failure: None,
            rec: Some(rec_info),
        }
    }
}

#[cfg(test)]
mod tests;
