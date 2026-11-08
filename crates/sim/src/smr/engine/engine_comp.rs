//! Compact-rule (Algorithm 5) round step.

use crate::smr::engine::stages::{amplify, apply_all, failed_step, sn_order_failure};
use crate::smr::observe::canon_cursor::{CanonCursor, canon_lookup};
use crate::smr::observe::canonical::Executed;
use crate::smr::observe::committed::useful_at_round_end;
use crate::smr::observe::full_prefix::{full_seqs, observe_full_prefix};
use crate::smr::observe::log_index::{LogCounts, LogIndex, log_window_pos};
use crate::smr::observe::round_metrics::smr_round_metrics;
use crate::smr::tracker::{
    CommandTracker, advance_inert_prefix, latch_coverage, pos_stats, spreading_list,
};
use crate::smr::*;
use protocol::compact::{
    ClientCommand, CompactNode, CompactReply, Entry, Log, SharedState, Triage,
};
use protocol::shared_state::ExecutedView;
use rand::Rng;
use std::collections::BTreeMap;
use std::sync::Arc;

impl SmrState {
    pub(in crate::smr) fn step_comp(&mut self, mask: &[bool]) -> SmrStepStatus {
        let n = self.n;
        let cfg = self.cfg;
        let amp = self.amp;
        let round = self.round;
        let arrivals = std::mem::take(&mut self.arrivals_pending);
        let SmrNodes::Comp(nodes) = &mut self.nodes else {
            unreachable!("dispatched on Comp");
        };
        // Nodes swap, never edit, their log and index, so these handles ARE the pre-step snapshot.
        let snap_logs: Vec<Option<Log>> =
            nodes.iter().map(|nd| nd.log_perm_arc().cloned()).collect();
        #[cfg(debug_assertions)]
        let snap_states: Vec<_> = if snap_logs.iter().any(Option::is_none) {
            nodes.iter().map(|nd| nd.shared_state().clone()).collect()
        } else {
            Vec::new()
        };
        // p. 19 useful at the round start: the row's `useful` count and the census read it, the Def 1.4
        // counters read `useful_at_round_end`.
        let useful_at_start: Vec<bool> =
            (0..n).map(|i| snap_logs[i].is_some() && !mask[i]).collect();
        let useful_total = useful_at_start.iter().filter(|&&u| u).count() as u32;

        // (b) client stage: resend every round until AckCommitted
        let mut delivered: Vec<(usize, ClientCommand)> = Vec::new();
        for &ti in &self.client_order {
            let t = &mut self.trackers[ti];
            if t.committed_ack_round.is_none() && round >= t.injection_round {
                let target = match t.target {
                    Some(id) => id as usize,
                    None => self.rng.random_range(0..n),
                };
                let outcome = if mask[target] {
                    AttemptOutcome::TargetBlocked
                } else {
                    t.injected_round.get_or_insert(round);
                    match nodes[target].on_client_command(&t.cc) {
                        Triage::Amplify => {
                            if t.delivered_round.is_none() {
                                t.delivered_round = Some(round);
                            }
                            delivered.push((target, t.cc));
                            AttemptOutcome::Amplified
                        }
                        Triage::AckCommitted => {
                            t.committed_ack_round = Some(round);
                            AttemptOutcome::AckCommitted
                        }
                        Triage::Ignore => AttemptOutcome::Ignored,
                    }
                };
                t.attempts.push(DeliveryAttempt {
                    round,
                    target: target as u32,
                    outcome,
                });
                // Alg 5 "if L_i ≠ ⊥ and sn(x) = sn(c)": the pool slot is drawable from the next round's stage (a0).
                if outcome == AttemptOutcome::AckCommitted {
                    let freed = t.client;
                    if let Some(pool) = &mut self.pool {
                        pool.free(freed);
                    }
                }
            }
        }

        // (c) amplify stage
        let partition = self.partition.as_deref();
        let appends = amplify(
            &mut self.rng,
            &mut self.trackers,
            delivered,
            |_, _| true,
            mask,
            partition,
            n,
            amp,
            round,
        );

        // (d) log-request stage: Alg 5 "i sends k requests ... and attaches a bit b_i", answered by `answer`
        let mut inboxes: Vec<Vec<CompactReply>> = Vec::with_capacity(n);
        let mut by_len: BTreeMap<(u64, usize), Arc<SharedState>> = BTreeMap::new();
        for i in 0..n {
            let b_i = nodes[i].b_i();
            let mut replies = Vec::with_capacity(cfg.k);
            for _ in 0..cfg.k {
                let target = self.rng.random_range(0..n);
                if !mask[i]
                    && !mask[target]
                    && same_component(partition, i, target)
                    && let Some(mut reply) = nodes[target].answer(b_i)
                {
                    if let Some(live) = &reply.s_j {
                        #[cfg(debug_assertions)]
                        assert_eq!(
                            live.as_ref(),
                            &snap_states[target],
                            "stage (d) read post-step state"
                        );
                        reply.s_j = Some(served_snapshot(&mut by_len, live));
                    }
                    replies.push(reply);
                }
            }
            inboxes.push(replies);
        }

        // (e) core step: all draws first, ascending id, reading only stage-(d) inbox lengths; applies are RNG-free.
        let choices: Vec<Option<Vec<usize>>> = (0..n)
            .map(|i| nodes[i].draw_reply_choice(inboxes[i].len(), &mut self.rng))
            .collect();
        apply_all(nodes, |i, node: &mut CompactNode| {
            node.step_chosen(
                &inboxes[i],
                &appends[i],
                round as u64,
                choices[i].as_deref(),
            );
        });

        // (f) observation: no RNG from here on
        let post_logs: Vec<Option<&protocol::chunked::ChunkSeq<protocol::compact::Timed>>> =
            nodes.iter().map(CompactNode::log_seq).collect();
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
        let pos_floor = self.checker.canonical_len() as u32;
        let useful_end = useful_at_round_end(post_logs.iter().map(Option::is_some), mask);

        if self.safety_ok {
            let executed: Vec<ExecutedView<'_>> = nodes
                .iter()
                .map(|nd| nd.shared_state().executed())
                .collect();
            if !self.checker.check_round(&executed, self.frontier) {
                self.safety_ok = false;
            }
        }
        if self.safety_ok {
            let canonical = self.checker.canonical();
            self.exec_dup
                .observe_canonical(canonical.start(), canonical.retained(), round);
        } else if self.exec_dup.tripped_round().is_none() {
            let executed: Vec<ExecutedView<'_>> = nodes
                .iter()
                .map(|nd| nd.shared_state().executed())
                .collect();
            // Alg 5 swaps S_i for a peer's only on the b_i = 1 (L_i = ⊥) branches.
            let replaced: Vec<bool> = snap_logs.iter().map(Option::is_none).collect();
            self.exec_dup.observe_nodes(&executed, &replaced, round);
        }
        {
            let executed: Vec<ExecutedView<'_>> = nodes
                .iter()
                .map(|nd| nd.shared_state().executed())
                .collect();
            self.committed
                .observe(&executed, self.checker.canonical(), round);
            self.committed
                .check_strong_safety(&useful_end, round, false);
        }
        let cursors_exact = self.safety_ok;
        if cursors_exact {
            let canonical = self.checker.canonical();
            self.canon_index
                .extend_first(canonical.start(), canonical.retained());
        }
        let canon_index = &self.canon_index;
        let canonical = self.checker.canonical();
        let release_line = self.release_line;
        if self.full_prefix {
            let age = self
                .safety_ok
                .then(|| {
                    let seqs = full_seqs(&post_logs, &executed_lens);
                    observe_full_prefix(
                        round,
                        canonical,
                        canon_index,
                        release_line,
                        &seqs,
                        &mut self.trackers,
                    )
                })
                .flatten();
            self.unsettled_ages.push(age);
        }

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

        // Executed lengths of the servers a commit must reach.
        let commit_floor = (0..n)
            .filter(|&i| useful_end[i])
            .map(|i| executed_lens[i] as u64)
            .min();

        // Whether every useful server has executed `entry` by this round's end.
        let committed_by_all = |entry: &Entry| {
            let Some(floor) = commit_floor else {
                return false;
            };
            if cursors_exact {
                return canon_index
                    .get_from(entry, release_line)
                    .is_some_and(|p| u64::from(p) < floor);
            }
            nodes.iter().enumerate().all(|(i, nd)| {
                if !useful_end[i] {
                    return true;
                }
                let executed = Executed {
                    view: nd.shared_state().executed(),
                    canonical,
                };
                let holds = executed.holds(entry);
                debug_assert_eq!(holds, executed.front_scan(entry).is_some());
                holds
            })
        };

        let drop_settled_spread = self.drop_settled_spread;
        let inert = |t: &CommandTracker| {
            t.committed_ack_round.is_some_and(|r| round > r + 1)
                && t.all_logs_round.is_some()
                && t.canon_cursor.is_none()
                && (!drop_settled_spread || t.spread_points == 0)
        };
        let was_live_from = self.inert_prefix;
        self.inert_prefix = advance_inert_prefix(&self.trackers, self.inert_prefix, inert);
        let live_from = self.inert_prefix;
        self.undated.extend(
            (was_live_from..live_from).filter(|&i| self.trackers[i].committed_round.is_none()),
        );
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
            if t.committed_ack_round.is_none_or(|r| round <= r + 1) {
                t.spread_points += 1;
                crate::smr::observe::census_count::window();
                // Outside the scoping branch: `live_floor` gates `forget_committed_prefix`.
                let opened = t
                    .canon_cursor
                    .get_or_insert_with(|| CanonCursor::new(pos_floor));
                live_floor = Some(live_floor.unwrap_or(u32::MAX).min(opened.floor()));
                let cursor_floor = u64::from(opened.floor());
                if observe_spread {
                    let useful_holders = useful_counts
                        .as_ref()
                        .expect("census on implies the counts were built")
                        .count(&entry);
                    debug_assert_eq!(
                        useful_holders,
                        snap_logs
                            .iter()
                            .zip(&useful_at_start)
                            .filter(|&(log, &u)| u
                                && log
                                    .as_ref()
                                    .is_some_and(|l| l.entries().iter().any(|e| e.entry == entry)))
                            .count() as u32
                    );
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
                    t.shed_attempts();
                }
            }
            latch_coverage(t, &entry, round, &post_index, &post_logs);
            if t.committed_round.is_none() && committed_by_all(&entry) {
                t.committed_round = Some(round);
            }
        }
        let trackers = &mut self.trackers;
        self.undated.retain(|&i| {
            let t = &mut trackers[i];
            if committed_by_all(&Entry::Cmd(t.cc)) {
                t.committed_round = Some(round);
            }
            t.committed_round.is_none()
        });

        let spreading = if drop_settled_spread {
            Vec::new()
        } else {
            spreading_list(
                &self.trackers,
                live_from,
                round,
                &post_index,
                &post_logs,
                |t| t.all_logs_round.is_none(),
            )
        };

        if round_metrics.nonbot_logs == 0 && self.dead.is_none() {
            self.dead = Some(round);
        }
        let glances = post_logs
            .iter()
            .zip(&executed_lens)
            .map(|(log, &ex)| NodeGlance {
                log_len: log.map(|l| l.len() as u32),
                executed_len: ex as u32,
                r: None,
            })
            .collect();
        drop(post_logs);

        if let Some((node, gap)) = nodes
            .iter()
            .enumerate()
            .find_map(|(i, nd)| nd.sn_order_violation().map(|gap| (i, gap)))
        {
            let failure = sn_order_failure(node, gap, round);
            let status = failed_step(&failure, round_metrics, mask, glances, spreading, None);
            self.failure = Some(failure);
            self.failed_status = Some(status.clone());
            return status;
        }

        // (g) truncation: after the walk; frozen once the safety latch is lost.
        if self.truncate_history && self.safety_ok {
            let floor = live_floor.map_or_else(|| self.checker.canonical_len() as u64, u64::from);
            self.frontier = self.frontier.max(floor);
            #[cfg(debug_assertions)]
            if let Some(floor) = live_floor {
                assert!(
                    self.frontier <= u64::from(floor),
                    "round {round}: the frontier {} passed a live cursor floor {floor}",
                    self.frontier
                );
            }
            for node in nodes.iter_mut() {
                let upto = self.frontier.min(node.shared_state().logical_len());
                node.forget_committed_prefix(upto);
                debug_assert_eq!(
                    node.shared_state().executed_offset(),
                    upto,
                    "round {round}: an offset is not min(frontier, own length)"
                );
            }
            // A cursorless undated command below the lowest offset is dated only in a round
            // with a useful server, so the release waits for one.
            if commit_floor.is_some() {
                let base = nodes
                    .iter()
                    .map(|nd| nd.shared_state().executed_offset())
                    .min()
                    .expect("at least one node");
                assert!(
                    base >= self.release_line,
                    "round {round}: the lowest executed offset {base} fell below the release \
                     line {}, which no server can re-present after forgetting",
                    self.release_line
                );
                self.release_line = base;
                self.checker.release_to(base);
                self.canon_index.release_to(base);
            }
        }

        SmrStepStatus {
            round,
            metrics: round_metrics,
            blocked: blocked_ids(mask),
            nodes: glances,
            spreading,
            dead: self.dead,
            failure: None,
            rec: None,
        }
    }
}

/// The S_j a stage-(d) reply carries: an equal-content snapshot already served this round, else the target's own.
fn served_snapshot(
    by_len: &mut BTreeMap<(u64, usize), Arc<SharedState>>,
    live: &Arc<SharedState>,
) -> Arc<SharedState> {
    let first = by_len
        .entry((live.logical_len(), live.sn_len()))
        .or_insert_with(|| Arc::clone(live));
    // Equal lengths do not imply equal content once servers diverge or nulls exist.
    if Arc::ptr_eq(first, live) || first.content_eq(live) {
        Arc::clone(first)
    } else {
        Arc::clone(live)
    }
}

#[cfg(test)]
mod tests;
