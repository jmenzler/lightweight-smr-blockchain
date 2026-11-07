//! Sans-IO round engine: `on_round_start` → any number of `on_message` → `on_round_end`.

use crate::NodeId;
use crate::digest::{fnv_chain, fnv_entries, fnv_ops, fnv_timed};
use crate::record::{PostState, RoundRecord, TrackedDigest};
use crate::rng::{derive_amplify_rng, derive_rng, draw_targets};
use crate::spec::{NodeRunSpec, ScenarioKind};
use crate::wire::{AckKind, Msg, ReplyPayload, encode};
use protocol::chunked::SealedMark;
use protocol::compact::{
    ClientCommand, CompactNode, CompactReply, Entry, SharedState, Timed, Triage,
};
use protocol::{Command, GossipNode, LogNode, MedianNode, PriorityNode};
use rand_chacha::ChaCha12Rng;
use std::sync::Arc;

enum ModeCore {
    Median(MedianNode),
    Gossip(GossipNode),
    Priority(PriorityNode),
    Extended(LogNode),
    Compact(CompactNode),
}

enum Snapshot {
    Bot,
    Value(u64),
    Log(Vec<Command>),
    Compact { log: Vec<Timed>, state: SharedState },
}

impl Snapshot {
    fn held(&self) -> bool {
        !matches!(self, Snapshot::Bot)
    }
}

#[derive(Default)]
struct Counters {
    req_sent: u32,
    req_recv: u32,
    rep_sent: u32,
    rep_recv: u32,
    app_sent: u32,
    app_recv: u32,
    app_blocked: u32,
    cli_recv: u32,
    acks: u32,
    bytes_out: u64,
    bytes_in: u64,
    late_rep: u32,
    late_app: u32,
    dropped_past: u32,
    late_cli: u32,
    early_cli: u32,
    cli_blocked: u32,
    encode_err: u32,
    transport_err: u32,
}

pub struct RoundEngine {
    node_id: NodeId,
    seed: u64,
    n: usize,
    k: usize,
    amp: usize,
    core: ModeCore,
    rng: ChaCha12Rng,
    masks: Vec<Vec<bool>>,
    round: u64,
    snapshot: Snapshot,
    slots: Vec<Option<ReplyPayload>>,
    /// (sender, client, sn, op); sorted into the sim's order at round end.
    appends_ext: Vec<(NodeId, u32, u64, Command)>,
    appends_comp: Vec<(NodeId, ClientCommand, u64)>,
    queued: Vec<Msg>,
    tracked_ops: Vec<u64>,
    /// `exec_chain[len]`: chained hash of the first `len` executed entries.
    exec_chain: Vec<u64>,
    exec_mark: Option<SealedMark<Entry>>,
    max_frame: u32,
    pad: u32,
    c: Counters,
    sn_order_violation: bool,
}

impl RoundEngine {
    pub fn new(spec: &NodeRunSpec, node_id: NodeId) -> Result<RoundEngine, String> {
        spec.validate()?;
        let (core, n, k, seed, schedule, max_rounds, amp) = match &spec.scenario {
            ScenarioKind::Smr(s) => {
                let scenario = sim::smr::SmrScenario::try_from(s.clone())?;
                let core = match scenario.proto {
                    sim::smr::Proto::Extended => {
                        ModeCore::Extended(LogNode::new(sim::smr::SEED_COMMAND, scenario.cfg))
                    }
                    sim::smr::Proto::Compact { t_commit_rounds } => ModeCore::Compact(
                        CompactNode::new(scenario.cfg, t_commit_rounds)
                            .with_repeated_commit(scenario.repeated_commit),
                    ),
                    sim::smr::Proto::Recovery { .. } => {
                        return Err("recovery proto is not supported in networked mode".into());
                    }
                };
                let amp = sim::smr::amp_count(scenario.sigma, scenario.n);
                (
                    core,
                    scenario.n,
                    scenario.cfg.k,
                    scenario.seed,
                    scenario.schedule,
                    scenario.max_rounds,
                    amp,
                )
            }
            ScenarioKind::Gossip(g) => {
                let scenario = spec.value_scenario()?;
                let initial = if (node_id as usize) < g.holders {
                    g.x
                } else {
                    g.x0
                };
                (
                    ModeCore::Gossip(GossipNode::new(initial, g.x, g.x0, scenario.cfg)),
                    scenario.n,
                    scenario.cfg.k,
                    scenario.seed,
                    scenario.schedule,
                    scenario.max_rounds,
                    0,
                )
            }
            ScenarioKind::Median(_) | ScenarioKind::Priority(_) => {
                let scenario = spec.value_scenario()?;
                let own = spec
                    .initial_values()?
                    .get(node_id as usize)
                    .copied()
                    .ok_or_else(|| format!("node id {node_id} outside 0..{}", scenario.n))?;
                let core = match &spec.scenario {
                    ScenarioKind::Median(_) => ModeCore::Median(match own {
                        Some(v) => MedianNode::new(v, scenario.cfg),
                        None => MedianNode::new_undecided(scenario.cfg),
                    }),
                    _ => ModeCore::Priority(PriorityNode::new(
                        own.expect("priority arm rejects undecided starts"),
                        scenario.cfg,
                    )),
                };
                (
                    core,
                    scenario.n,
                    scenario.cfg.k,
                    scenario.seed,
                    scenario.schedule,
                    scenario.max_rounds,
                    0,
                )
            }
        };
        if node_id as usize >= n {
            return Err(format!("node id {node_id} outside 0..{n}"));
        }
        let mut mask_rng = derive_rng(seed, "blocking", 0);
        let masks = sim::mask_schedule(&schedule, n, max_rounds, &mut mask_rng);
        Ok(RoundEngine {
            node_id,
            seed,
            n,
            k,
            amp,
            core,
            rng: derive_rng(seed, "node", node_id),
            masks,
            round: 0,
            snapshot: Snapshot::Bot,
            slots: Vec::new(),
            appends_ext: Vec::new(),
            appends_comp: Vec::new(),
            queued: Vec::new(),
            tracked_ops: spec.tracked_ops.clone(),
            exec_chain: vec![sim::digest::FNV_BASIS],
            exec_mark: None,
            max_frame: spec.max_frame_bytes,
            pad: spec.payload_bytes,
            c: Counters::default(),
            sn_order_violation: false,
        })
    }

    /// Adds transport failures the shell observed since the last round end.
    pub fn add_transport_err(&mut self, count: u32) {
        self.c.transport_err += count;
    }

    /// True for a client command tagged for the next round: the transport must hold it until then.
    pub fn defers(&self, msg: &Msg) -> bool {
        matches!(msg, Msg::ClientCmd { round, .. } if *round == self.round + 1)
    }

    pub fn round(&self) -> u64 {
        self.round
    }

    fn blocked_now(&self) -> bool {
        self.masks
            .get((self.round.max(1) - 1) as usize)
            .map(|row| row[self.node_id as usize])
            .expect("mask schedule covers every round of the horizon")
    }

    fn frame_len(&mut self, msg: &Msg) -> u64 {
        match encode(msg, self.max_frame, self.pad) {
            Ok(f) => f.len() as u64,
            Err(e) => {
                eprintln!("node {}: encode failed: {e}", self.node_id);
                self.c.encode_err += 1;
                0
            }
        }
    }

    fn take_snapshot(&self) -> Snapshot {
        match &self.core {
            ModeCore::Median(node) => node.answer().map_or(Snapshot::Bot, Snapshot::Value),
            ModeCore::Gossip(node) => node.answer().map_or(Snapshot::Bot, Snapshot::Value),
            ModeCore::Priority(node) => node.answer().map_or(Snapshot::Bot, Snapshot::Value),
            ModeCore::Extended(node) => node
                .log()
                .map_or(Snapshot::Bot, |l| Snapshot::Log(l.to_vec())),
            ModeCore::Compact(node) => match node.log_entries() {
                Some(log) => Snapshot::Compact {
                    log: log.to_vec(),
                    // cloned now: reading it after the step would serve post-step state
                    state: node.shared_state().clone(),
                },
                None => Snapshot::Bot,
            },
        }
    }

    /// Panics when called more than `max_rounds` times: the blocking masks cover the horizon only.
    pub fn on_round_start(&mut self) -> Vec<(NodeId, Msg)> {
        self.round += 1;
        self.c = Counters::default();
        self.slots = vec![None; self.k];
        self.appends_ext.clear();
        self.appends_comp.clear();
        self.snapshot = self.take_snapshot();

        let mut out = Vec::new();
        let due: Vec<Msg> = {
            let round = self.round;
            let (now, later): (Vec<Msg>, Vec<Msg>) = std::mem::take(&mut self.queued)
                .into_iter()
                .partition(|m| msg_round(m) == round);
            self.queued = later;
            now
        };
        for msg in due {
            out.extend(self.on_message(msg));
        }

        // drawn before the blocked check, so a blocked round takes the same k target draws as an open one
        let targets = draw_targets(&mut self.rng, self.n, self.k);
        if self.blocked_now() {
            return Vec::new();
        }
        let needs_state = matches!(self.core, ModeCore::Compact(_)) && !self.snapshot.held();
        for (slot, target) in targets.into_iter().enumerate() {
            let msg = Msg::PullRequest {
                round: self.round,
                from: self.node_id,
                slot: slot as u16,
                needs_state,
            };
            self.c.req_sent += 1;
            self.c.bytes_out += self.frame_len(&msg);
            out.push((target, msg));
        }
        out
    }

    fn amplify(&mut self, client: u32, sn: u64, op: u64) -> Vec<(NodeId, Msg)> {
        let mut rng = derive_amplify_rng(self.seed, self.node_id, self.round, client, sn);
        let targets = draw_targets(&mut rng, self.n, self.amp);
        let mut out = Vec::with_capacity(targets.len());
        for target in targets {
            let msg = Msg::Append {
                round: self.round,
                from: self.node_id,
                client,
                sn,
                op,
            };
            self.c.app_sent += 1;
            self.c.bytes_out += self.frame_len(&msg);
            out.push((target, msg));
        }
        out
    }

    fn on_client_cmd(&mut self, client: u32, sn: u64, op: u64) -> Vec<(NodeId, Msg)> {
        self.c.cli_recv += 1;
        let ack = |kind: AckKind, round: u64| Msg::ClientAck {
            round,
            client,
            sn,
            kind,
        };
        match &mut self.core {
            ModeCore::Median(_) | ModeCore::Gossip(_) | ModeCore::Priority(_) => Vec::new(),
            ModeCore::Extended(node) => {
                let wants = node.wants_amplify(op);
                let mut out = vec![(client, ack(AckKind::Delivered, self.round))];
                self.c.acks += 1;
                if wants {
                    out.extend(self.amplify(client, sn, op));
                }
                out
            }
            ModeCore::Compact(node) => {
                let cc = ClientCommand { client, sn, op };
                let triage = node.on_client_command(&cc);
                // deviation: see node/networked-node-architecture.md (F16)
                if !matches!(triage, Triage::Ignore) {
                    self.c.acks += 1;
                }
                match triage {
                    Triage::Amplify => {
                        let mut out = vec![(client, ack(AckKind::Amplified, self.round))];
                        out.extend(self.amplify(client, sn, op));
                        // deviation: see node/networked-node-architecture.md (F12)
                        out
                    }
                    Triage::AckCommitted => vec![(client, ack(AckKind::AckCommitted, self.round))],
                    Triage::Ignore => vec![(client, ack(AckKind::Ignored, self.round))],
                }
            }
        }
    }

    pub fn on_message(&mut self, msg: Msg) -> Vec<(NodeId, Msg)> {
        // deviation: see node/networked-node-architecture.md (F14)
        if let Msg::ClientCmd {
            round,
            client,
            sn,
            op,
        } = msg
        {
            if round < self.round {
                self.c.late_cli += 1;
                return Vec::new();
            }
            if round > self.round {
                self.c.early_cli += 1;
                return Vec::new();
            }
            if self.blocked_now() {
                self.c.cli_blocked += 1;
                return Vec::new();
            }
            return self.on_client_cmd(client, sn, op);
        }

        if let Some(from) = sender(&msg)
            && from as usize >= self.n
        {
            self.c.transport_err += 1;
            return Vec::new();
        }

        let r = msg_round(&msg);
        if r == self.round + 1 {
            self.queued.push(msg);
            return Vec::new();
        }
        if r != self.round {
            match msg {
                Msg::PullReply { .. } if r < self.round => self.c.late_rep += 1,
                Msg::Append { .. } if r < self.round => self.c.late_app += 1,
                _ => self.c.dropped_past += 1,
            }
            return Vec::new();
        }
        let blocked = self.blocked_now();
        match &msg {
            Msg::PullRequest { .. } => self.c.req_recv += 1,
            Msg::Append { .. } if blocked => self.c.app_blocked += 1,
            _ => {}
        }
        if blocked {
            return Vec::new();
        }
        match msg {
            Msg::PullRequest {
                from,
                slot,
                needs_state,
                ..
            } => {
                self.c.bytes_in += self.frame_len(&Msg::PullRequest {
                    round: self.round,
                    from,
                    slot,
                    needs_state,
                });
                let payload = match &self.snapshot {
                    Snapshot::Bot => None,
                    Snapshot::Value(v) => Some(ReplyPayload::Value(*v)),
                    Snapshot::Log(log) => Some(ReplyPayload::Log(log.clone())),
                    Snapshot::Compact { log, state } => Some(ReplyPayload::Compact {
                        log: log.clone(),
                        state: needs_state.then(|| state.clone()),
                    }),
                };
                match payload {
                    Some(payload) => {
                        let reply = Msg::PullReply {
                            round: self.round,
                            from: self.node_id,
                            slot,
                            payload,
                        };
                        self.c.rep_sent += 1;
                        self.c.bytes_out += self.frame_len(&reply);
                        vec![(from, reply)]
                    }
                    // x_i = ⊥: no reply (Alg 1 "if x_i ≠ ⊥ then ... send x_i back")
                    None => Vec::new(),
                }
            }
            Msg::PullReply { slot, payload, .. } => {
                self.c.bytes_in += self.frame_len(&Msg::PullReply {
                    round: self.round,
                    from: 0,
                    slot,
                    payload: payload.clone(),
                });
                let slot = slot as usize;
                if slot < self.slots.len() && self.slots[slot].is_none() {
                    self.slots[slot] = Some(payload);
                    self.c.rep_recv += 1;
                }
                Vec::new()
            }
            Msg::Append {
                round,
                from,
                client,
                sn,
                op,
            } => {
                self.c.app_recv += 1;
                self.c.bytes_in += self.frame_len(&Msg::Append {
                    round,
                    from: 0,
                    client,
                    sn,
                    op,
                });
                match &self.core {
                    ModeCore::Extended(_) => self.appends_ext.push((from, client, sn, op)),
                    ModeCore::Compact(_) => {
                        self.appends_comp
                            .push((from, ClientCommand { client, sn, op }, round))
                    }
                    _ => {}
                }
                Vec::new()
            }
            Msg::ClientAck { .. } | Msg::RoundDone { .. } | Msg::RoundGo { .. } => Vec::new(),
            Msg::ClientCmd { .. } => unreachable!("handled above"),
        }
    }

    pub fn on_round_end(&mut self) -> RoundRecord {
        let collected: Vec<ReplyPayload> = self.slots.iter().flatten().cloned().collect();
        let replies = collected.len() as u16;
        let values = payload_values(&collected);
        match &mut self.core {
            ModeCore::Median(node) => node.step(&values, &mut self.rng),
            ModeCore::Gossip(node) => node.step(&values, &mut self.rng),
            ModeCore::Priority(node) => node.step(&values, &mut self.rng),
            ModeCore::Extended(node) => {
                let logs: Vec<Arc<Vec<Command>>> = collected
                    .iter()
                    .filter_map(|p| match p {
                        ReplyPayload::Log(l) => Some(Arc::new(l.clone())),
                        _ => None,
                    })
                    .collect();
                self.appends_ext
                    .sort_by_key(|&(from, client, sn, _)| (from, client, sn));
                let appends: Vec<Command> = self.appends_ext.iter().map(|a| a.3).collect();
                node.step(&logs, &appends, &mut self.rng);
            }
            ModeCore::Compact(node) => {
                let replies: Vec<CompactReply> = collected
                    .iter()
                    .filter_map(|p| match p {
                        ReplyPayload::Compact { log, state } => Some(CompactReply::from_log(
                            Arc::new(protocol::chunked::ChunkSeq::from_vec(log.clone())),
                            state.clone().map(Arc::new),
                        )),
                        _ => None,
                    })
                    .collect();
                if !self.sn_order_violation {
                    self.appends_comp
                        .sort_by_key(|&(from, cc, _)| (from, cc.client, cc.sn));
                    let appends: Vec<(ClientCommand, u64)> =
                        self.appends_comp.iter().map(|a| (a.1, a.2)).collect();
                    node.step(&replies, &appends, self.round, &mut self.rng);
                    if let Some(gap) = node.sn_order_violation() {
                        eprintln!(
                            "node {}: round {}: the execute-once state machine refused an \
                             out-of-sn-order commit ({gap:?}); the node is frozen",
                            self.node_id, self.round
                        );
                        self.sn_order_violation = true;
                    }
                }
            }
        }

        let post = match &self.core {
            ModeCore::Median(node) => PostState::Value(node.answer()),
            ModeCore::Gossip(node) => PostState::Value(node.answer()),
            ModeCore::Priority(node) => PostState::Value(node.answer()),
            ModeCore::Extended(node) => PostState::Log {
                log_len: node.log().map(|l| l.len()),
                exec_len: 0,
                log_hash: node.log().map(|l| fnv_ops(l.iter().copied())),
            },
            ModeCore::Compact(node) => PostState::Log {
                log_len: node.log_entries().map(|l| l.len()),
                exec_len: node.shared_state().untruncated().len(),
                log_hash: node.log_entries().map(|l| fnv_timed(l.iter())),
            },
        };
        let exec_prefix = self.exec_prefix_delta();
        let tracked = self.tracked_digests();
        let c = std::mem::take(&mut self.c);
        RoundRecord {
            r: self.round,
            blocked: self.blocked_now(),
            snap_held: self.snapshot.held(),
            post,
            replies,
            req_sent: c.req_sent,
            req_recv: c.req_recv,
            rep_sent: c.rep_sent,
            rep_recv: c.rep_recv,
            app_sent: c.app_sent,
            app_recv: c.app_recv,
            app_blocked: c.app_blocked,
            cli_recv: c.cli_recv,
            acks: c.acks,
            bytes_out: c.bytes_out,
            bytes_in: c.bytes_in,
            late_rep: c.late_rep,
            late_app: c.late_app,
            dropped_past: c.dropped_past,
            late_cli: c.late_cli,
            early_cli: c.early_cli,
            cli_blocked: c.cli_blocked,
            sn_order_violation: self.sn_order_violation,
            encode_err: c.encode_err,
            transport_err: c.transport_err,
            exec_prefix,
            t_last_reply_us: 0,
            t_step_done_us: 0,
            t_step_compute_us: 0,
            tracked,
        }
    }

    /// (length, chained hash) for every prefix whose hash is new or changed this round.
    fn exec_prefix_delta(&mut self) -> Vec<(usize, u64)> {
        let ModeCore::Compact(node) = &self.core else {
            return Vec::new();
        };
        let executed = node.shared_state().untruncated();
        // Alg 5 S_i := S_j (p. 26) may rewrite S_i below any prefix not provably still held.
        let start = match &self.exec_mark {
            Some(mark) if executed.holds(mark) => mark.end() as usize,
            _ => 0,
        };
        let stale = self.exec_chain.split_off(start + 1);
        let mut out = Vec::new();
        for (p, entry) in executed.iter_from(start as u64).enumerate() {
            let hash = fnv_chain(*self.exec_chain.last().expect("holds the basis"), entry);
            self.exec_chain.push(hash);
            if stale.get(p) != Some(&hash) {
                out.push((start + p + 1, hash));
            }
        }
        self.exec_mark = executed.last_sealed();
        out
    }

    fn tracked_digests(&self) -> Vec<TrackedDigest> {
        self.tracked_ops
            .iter()
            .map(|&op| match &self.core {
                ModeCore::Extended(node) => {
                    let (pos, hash) = node
                        .log()
                        .and_then(|log| {
                            log.iter()
                                .position(|&c| c == op)
                                .map(|p| (Some(p), Some(fnv_ops(log[..=p].iter().copied()))))
                        })
                        .unwrap_or((None, None));
                    TrackedDigest {
                        op,
                        present: pos.is_some(),
                        in_log: pos.is_some(),
                        pos,
                        prefix_hash: hash,
                    }
                }
                ModeCore::Compact(node) => {
                    let state = node.shared_state();
                    // the witness is rooted at index 0, so states here must never be truncated
                    let executed = state.untruncated();
                    let exec_pos = executed.iter().position(|e| e.op() == Some(op));
                    let in_log = node
                        .log_entries()
                        .is_some_and(|log| log.iter().any(|t| t.entry.op() == Some(op)));
                    let (pos, hash) = match exec_pos {
                        Some(p) => (Some(p), Some(fnv_entries(executed.iter().take(p + 1)))),
                        None => match node.log_entries().and_then(|log| {
                            log.iter().position(|t| t.entry.op() == Some(op)).map(|w| {
                                // x_d, x_0 and ⊥ entries never reach the executed sequence, so they
                                // take no position; counting them would make the digest depend on
                                // whether this server has committed the command yet
                                let mut prefix: Vec<&Entry> = executed.iter().collect();
                                prefix.extend(
                                    log[..=w]
                                        .iter()
                                        .map(|t| &t.entry)
                                        .filter(|e| e.op().is_some()),
                                );
                                (prefix.len() - 1, fnv_entries(prefix.into_iter()))
                            })
                        }) {
                            Some((p, h)) => (Some(p), Some(h)),
                            None => (None, None),
                        },
                    };
                    TrackedDigest {
                        op,
                        present: pos.is_some(),
                        in_log,
                        pos,
                        prefix_hash: hash,
                    }
                }
                _ => TrackedDigest {
                    op,
                    present: false,
                    in_log: false,
                    pos: None,
                    prefix_hash: None,
                },
            })
            .collect()
    }
}

fn payload_values(collected: &[ReplyPayload]) -> Vec<u64> {
    collected
        .iter()
        .filter_map(|p| match p {
            ReplyPayload::Value(v) => Some(*v),
            _ => None,
        })
        .collect()
}

/// The server id a peer-to-peer message claims to come from.
fn sender(msg: &Msg) -> Option<NodeId> {
    match msg {
        Msg::PullRequest { from, .. } | Msg::PullReply { from, .. } | Msg::Append { from, .. } => {
            Some(*from)
        }
        Msg::ClientCmd { .. }
        | Msg::ClientAck { .. }
        | Msg::RoundDone { .. }
        | Msg::RoundGo { .. } => None,
    }
}

fn msg_round(msg: &Msg) -> u64 {
    match msg {
        Msg::PullRequest { round, .. }
        | Msg::PullReply { round, .. }
        | Msg::Append { round, .. }
        | Msg::ClientCmd { round, .. }
        | Msg::ClientAck { round, .. }
        | Msg::RoundDone { round, .. }
        | Msg::RoundGo { round } => *round,
    }
}
