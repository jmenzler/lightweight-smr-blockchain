//! Extended-rule (Algorithm 3) round step.

use crate::smr::*;
use protocol::LogNode;
use protocol::extended::Command;
use rand::Rng;
use std::sync::Arc;

fn lcp_over<'a, I: Iterator<Item = &'a [Command]>>(mut logs: I) -> u32 {
    let Some(first) = logs.next() else { return 0 };
    let mut lcp = first.len();
    for log in logs {
        let common = first
            .iter()
            .zip(log.iter())
            .take_while(|(a, b)| a == b)
            .count();
        lcp = lcp.min(common);
        if lcp == 0 {
            break;
        }
    }
    lcp as u32
}

impl SmrState {
    pub(in crate::smr) fn step_ext(&mut self, mask: &[bool]) -> SmrStepStatus {
        let n = self.n;
        let cfg = self.cfg;
        let amp = self.amp;
        let round = self.round;
        let arrivals = std::mem::take(&mut self.arrivals_pending);
        let SmrNodes::Ext(nodes) = &mut self.nodes else {
            unreachable!("dispatched on Ext");
        };
        let snapshot: Vec<Option<Arc<Vec<Command>>>> = nodes.iter().map(LogNode::answer).collect();
        let useful: Vec<bool> = (0..n).map(|i| snapshot[i].is_some() && !mask[i]).collect();
        let useful_total = useful.iter().filter(|&&u| u).count() as u32;

        // (b) client stage
        let mut delivered: Vec<(usize, Command)> = Vec::new();
        for &ti in &self.client_order {
            let t = &mut self.trackers[ti];
            if t.delivered_round.is_none() && round >= t.injection_round {
                // A pinned target draws nothing; the random arm is the historical draw line.
                let target = match t.target {
                    Some(id) => id as usize,
                    None => self.rng.random_range(0..n),
                };
                let outcome = if mask[target] {
                    AttemptOutcome::TargetBlocked
                } else {
                    t.injected_round = Some(round);
                    t.delivered_round = Some(round);
                    delivered.push((target, t.op));
                    AttemptOutcome::Delivered
                };
                t.attempts.push(DeliveryAttempt {
                    round,
                    target: target as u32,
                    outcome,
                });
            }
        }

        // (c) amplify stage
        let partition = self.partition.as_deref();
        let mut appends: Vec<Vec<Command>> = vec![Vec::new(); n];
        delivered.sort_by_key(|&(server, _)| server);
        for &(server, x) in &delivered {
            if nodes[server].wants_amplify(x) {
                let mut receivers: Vec<u32> = Vec::new();
                for _ in 0..amp {
                    let target = self.rng.random_range(0..n);
                    if !mask[target] && same_component(partition, server, target) {
                        appends[target].push(x);
                        receivers.push(target as u32);
                    }
                }
                receivers.sort_unstable();
                receivers.dedup();
                let tracker = self
                    .trackers
                    .iter_mut()
                    .find(|t| t.op == x)
                    .expect("tracked op");
                tracker.amp_receivers = receivers;
            }
        }

        // (d) log-request stage
        let mut inboxes: Vec<Vec<Arc<Vec<Command>>>> = Vec::with_capacity(n);
        for i in 0..n {
            let mut replies = Vec::with_capacity(cfg.k);
            for _ in 0..cfg.k {
                let target = self.rng.random_range(0..n);
                if !mask[i]
                    && !mask[target]
                    && same_component(partition, i, target)
                    && let Some(log) = &snapshot[target]
                {
                    replies.push(Arc::clone(log));
                }
            }
            inboxes.push(replies);
        }

        // (e) core step
        for (i, node) in nodes.iter_mut().enumerate() {
            node.step(&inboxes[i], &appends[i], &mut self.rng);
        }

        // (f) observation: no RNG from here on
        let post: Vec<Option<&[Command]>> = nodes.iter().map(LogNode::log).collect();
        let nonbot_logs = post.iter().filter(|l| l.is_some()).count() as u32;
        let round_metrics = SmrRoundMetrics {
            nonbot_logs,
            blocked: mask.iter().filter(|&&b| b).count() as u32,
            useful: useful_total,
            distinct_logs: post
                .iter()
                .flatten()
                .collect::<std::collections::HashSet<_>>()
                .len() as u32,
            max_log_len: post.iter().flatten().map(|l| l.len()).max().unwrap_or(0) as u32,
            min_executed_len: 0,
            max_executed_len: 0,
            useful_executed: None,
            arrivals,
            lcp_len: lcp_over(post.iter().flatten().copied()),
        };
        self.metrics.push(round_metrics);

        let observe_spread = self.observe_spread;
        for t in self.trackers.iter_mut() {
            t.observe(
                round,
                &snapshot,
                &useful,
                useful_total,
                &post,
                observe_spread,
            );
        }
        let spreading = self
            .trackers
            .iter()
            .filter(|t| t.all_logs_round.is_none() && round >= t.injection_round)
            .map(|t| SpreadingCommand {
                client: t.client,
                holders: post
                    .iter()
                    .enumerate()
                    .filter(|(_, log)| log.is_some_and(|l| l.contains(&t.op)))
                    .map(|(id, _)| id as u32)
                    .collect(),
            })
            .collect();

        if nonbot_logs == 0 && self.dead.is_none() {
            self.dead = Some(round);
        }
        let glances = post
            .iter()
            .map(|log| NodeGlance {
                log_len: log.map(|l| l.len() as u32),
                executed_len: 0,
                r: None,
            })
            .collect();
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
