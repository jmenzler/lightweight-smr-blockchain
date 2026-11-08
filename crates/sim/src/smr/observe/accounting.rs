//! Structural byte accounting over a live `SmrState`; instrumentation only, draws no RNG.

use crate::smr::*;
use protocol::certificates::{ServerCertState, StoredCommand};
use protocol::compact::{Entry, Timed};
use std::collections::BTreeMap;

const HASH: usize = 32;

/// One accounting pass: retained bytes by category plus sharing factors.
#[derive(Debug, Clone, Default)]
pub struct Accounting {
    pub round: usize,
    pub rows: BTreeMap<&'static str, u64>,
    /// Distinct `Arc<Checkpoint>` allocations; `checkpoint.*` rows are per fork class, not per node.
    pub distinct_checkpoints: usize,
    pub distinct_logs: usize,
    pub distinct_log_chunks: usize,
    /// Distinct sealed chunks among node states only; not summable with checkpoint-state chunks.
    pub distinct_state_chunks: usize,
    pub distinct_states: usize,
    pub distinct_node_states: usize,
    pub nodes: usize,
    pub distinct_clients: usize,
    pub cert_stored_commands: usize,
    pub transient_per_round: u64,
    /// Occupancy of bytes already charged in `node.log.*`; kept out of `rows` to avoid double-counting.
    pub log_len_bytes: u64,
    pub perm_len_bytes: u64,
    /// False for compact, whose node, log and checkpoint rows are not walked.
    pub walked_nodes: bool,
}

impl Accounting {
    /// The retained floor; excludes `.distinct` rows, which re-count per-holder bytes.
    pub fn retained_total(&self) -> u64 {
        self.rows
            .iter()
            .filter(|(k, _)| !k.ends_with(".distinct"))
            .map(|(_, b)| *b)
            .sum()
    }

    /// Per-holder over per-allocation state bytes, over node states only.
    pub fn state_duplication(&self) -> f64 {
        let per_holder = self.rows.get("node.state.executed").copied().unwrap_or(0)
            + self.rows.get("node.state.sn").copied().unwrap_or(0);
        let distinct = self
            .rows
            .get("node.state.executed.distinct")
            .copied()
            .unwrap_or(0)
            + self
                .rows
                .get("node.state.sn.distinct")
                .copied()
                .unwrap_or(0);
        if distinct == 0 {
            return 1.0;
        }
        per_holder as f64 / distinct as f64
    }

    pub fn log_capacity_bytes(&self) -> u64 {
        self.rows.get("node.log.entries").copied().unwrap_or(0)
            + self.rows.get("node.log.perm").copied().unwrap_or(0)
    }

    pub fn ranked(&self) -> Vec<(&'static str, u64)> {
        let mut v: Vec<(&'static str, u64)> = self.rows.iter().map(|(k, b)| (*k, *b)).collect();
        v.sort_by_key(|&(_, b)| std::cmp::Reverse(b));
        v
    }
}

fn map_bytes<K, V>(len: usize) -> u64 {
    ((len * (size_of::<K>() + size_of::<V>())) as u64 * 3) / 2
}

fn chain_bytes(s: &StoredCommand) -> u64 {
    (s.chain.len() * HASH) as u64
}

fn cert_bytes(certs: Option<&ServerCertState>, clients: &[u32]) -> (u64, u64, usize) {
    let Some(certs) = certs else {
        return (0, 0, 0);
    };
    let forest = (certs.forest().peaks().len() * size_of::<(u32, [u8; 32], u64)>()) as u64;
    let mut windows = 0u64;
    let mut held = 0usize;
    let mut stored = 0usize;
    for &client in clients {
        if let Some((prev, newest)) = certs.last_two(client) {
            held += 1;
            stored += 1;
            windows += chain_bytes(newest);
            if let Some(p) = prev {
                stored += 1;
                windows += chain_bytes(p);
            }
        }
    }
    windows += map_bytes::<u32, (Option<StoredCommand>, StoredCommand)>(held);
    (forest, windows, stored)
}

fn state_bytes(state: &protocol::compact::SharedState) -> (u64, u64) {
    let executed = (state.retained_len() * size_of::<Entry>()) as u64;
    let auto_extent = state
        .sn_iter()
        .filter(|&(c, _)| c >= types::AUTO_CLIENT_BASE)
        .map(|(c, _)| u64::from(c - types::AUTO_CLIENT_BASE) + 1)
        .max()
        .unwrap_or(0);
    let manual = state
        .sn_iter()
        .filter(|&(c, _)| c < types::AUTO_CLIENT_BASE)
        .count();
    let sn = auto_extent * size_of::<u64>() as u64 + map_bytes::<u32, u64>(manual);
    (executed, sn)
}

/// One server's bytes as if it owned every structure it references: the simulator shares
/// allocations between nodes (log chunks, executed chunks, interned checkpoints), a real server
/// would not, so nothing here is divided by a sharing factor. Structural estimates (element size ×
/// count, maps at 1.5× their entries), not allocator RSS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct NodeBytes {
    /// L_i: its entries plus the median rule's permutation index.
    pub log: u64,
    /// S_i's executed sequence; the simulator represents the state machine by it.
    pub executed: u64,
    /// S_i's per-client sequence-number map.
    pub sn: u64,
    /// Recovery's C_i: its S (executed sequence and sn map) and P.
    pub checkpoint: u64,
    /// §5 certificate state: the Merkle forest's peaks and the last-two windows per client.
    pub certs: u64,
}

impl NodeBytes {
    pub fn total(&self) -> u64 {
        self.log + self.executed + self.sn + self.checkpoint + self.certs
    }
}

/// Per-node bytes over the servers holding a log; `median_node` is the median server's breakdown.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Footprint {
    pub counted: usize,
    /// Upper median of the per-node totals.
    pub median: u64,
    pub max: u64,
    pub median_node: NodeBytes,
}

/// `None` entries are ⊥ servers, left out; `None` when every server is ⊥.
pub fn footprint(per_node: &[Option<NodeBytes>]) -> Option<Footprint> {
    let mut held: Vec<NodeBytes> = per_node.iter().flatten().copied().collect();
    if held.is_empty() {
        return None;
    }
    held.sort_by_key(NodeBytes::total);
    let median_node = held[held.len() / 2];
    Some(Footprint {
        counted: held.len(),
        median: median_node.total(),
        max: held.last().map_or(0, NodeBytes::total),
        median_node,
    })
}

/// A server's §5 certificate state in full: peaks plus every stored window and its map entry.
pub fn cert_state_bytes(certs: &ServerCertState, clients: &[u32]) -> u64 {
    let (forest, windows, _) = cert_bytes(Some(certs), clients);
    forest + windows
}

/// One pass over shared allocations: a state, log, chunk or checkpoint is charged on first sight.
#[derive(Default)]
struct SharedWalk {
    bytes: u64,
    states: std::collections::HashSet<usize>,
    chunks: std::collections::HashSet<usize>,
    logs: std::collections::HashSet<usize>,
    perms: std::collections::HashSet<usize>,
    cps: std::collections::HashSet<usize>,
}

impl SharedWalk {
    fn state(&mut self, state: &protocol::compact::SharedState) {
        if !self.states.insert(std::ptr::from_ref(state) as usize) {
            return;
        }
        self.bytes += map_bytes::<u32, u64>(state.sn_len());
        let k = protocol::shared_state::ExecSeq::K;
        for ptr in state.executed_chunk_ptrs() {
            if self.chunks.insert(ptr as usize) {
                self.bytes += (k * size_of::<Entry>()) as u64;
            }
        }
        let tail = state.retained_len() - state.executed_sealed_count() * k;
        self.bytes += (tail * size_of::<Entry>()) as u64;
    }

    fn log(&mut self, log: Option<&protocol::log::Log>) {
        let Some(l) = log else { return };
        if self.perms.insert(std::sync::Arc::as_ptr(l.perm()) as usize) {
            self.bytes += (l.perm().len() * size_of::<u32>()) as u64;
        }
        let entries = l.entries();
        if !self.logs.insert(std::sync::Arc::as_ptr(entries) as usize) {
            return;
        }
        let k = protocol::compact::LogSeq::K;
        for ptr in entries.chunk_ptrs() {
            if self.chunks.insert(ptr as usize) {
                self.bytes += (k * size_of::<Timed>()) as u64;
            }
        }
        let tail = entries.retained_len() - entries.sealed_count() * k;
        self.bytes += (tail * size_of::<Timed>()) as u64;
    }
}

// Both walks below count in one set of units, so their ratio is sharing alone: element size ×
// elements physically held (a chunked sequence's retained length), sn maps and §5 windows as maps.
fn standalone_state_bytes(state: &protocol::compact::SharedState) -> (u64, u64) {
    (
        (state.retained_len() * size_of::<Entry>()) as u64,
        map_bytes::<u32, u64>(state.sn_len()),
    )
}

fn standalone_log_bytes(log: Option<&protocol::log::Log>) -> u64 {
    log.map_or(0, |l| {
        (l.entries().retained_len() * size_of::<Timed>() + l.perm().len() * size_of::<u32>()) as u64
    })
}

fn pre_bytes(cp: &protocol::recovery::Checkpoint) -> u64 {
    cp.p.as_ref()
        .map_or(0, |p| (p.len() * size_of::<Timed>()) as u64)
}

impl SmrState {
    /// The clients the run has seen, sorted; the §5 windows are keyed by them.
    pub fn tracked_clients(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.trackers.iter().map(|t| t.client).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Every server's standalone bytes, ⊥ servers too (log 0; they still hold S_i and C_i), and
    /// whether it holds a log.
    fn standalone_all(&self) -> Option<Vec<(NodeBytes, bool)>> {
        match &self.nodes {
            SmrNodes::Ext(_) => None,
            SmrNodes::Comp(nodes) => Some(
                nodes
                    .iter()
                    .map(|nd| {
                        let (executed, sn) = standalone_state_bytes(nd.shared_state());
                        let bytes = NodeBytes {
                            log: standalone_log_bytes(nd.log_perm_arc()),
                            executed,
                            sn,
                            ..Default::default()
                        };
                        (bytes, nd.log_perm_arc().is_some())
                    })
                    .collect(),
            ),
            SmrNodes::Rec(nodes) => {
                let clients = self.tracked_clients();
                // interned checkpoints repeat across nodes: walk each allocation once
                let mut per_cp: std::collections::HashMap<usize, (u64, u64)> =
                    std::collections::HashMap::new();
                Some(
                    nodes
                        .iter()
                        .map(|nd| {
                            let (executed, sn) = standalone_state_bytes(nd.shared_state());
                            let cp = nd.checkpoint();
                            let (checkpoint, certs) = *per_cp
                                .entry(std::ptr::from_ref(cp) as usize)
                                .or_insert_with(|| {
                                    let (cp_executed, cp_sn) = standalone_state_bytes(&cp.s);
                                    let certs = cp
                                        .certs
                                        .as_ref()
                                        .map_or(0, |c| cert_state_bytes(c, &clients));
                                    (cp_executed + cp_sn + pre_bytes(cp), certs)
                                });
                            let bytes = NodeBytes {
                                log: standalone_log_bytes(nd.log_perm_arc()),
                                executed,
                                sn,
                                checkpoint,
                                certs,
                            };
                            (bytes, nd.log_perm_arc().is_some())
                        })
                        .collect(),
                )
            }
        }
    }

    /// Per-node standalone bytes (see [`NodeBytes`]), `None` for a ⊥ server; reads only. Compact
    /// nodes carry no certificate state (the §5 layer lives beside the engine), extended is not
    /// walked.
    pub fn node_bytes(&self) -> Option<Vec<Option<NodeBytes>>> {
        Some(
            self.standalone_all()?
                .into_iter()
                .map(|(bytes, holds_log)| holds_log.then_some(bytes))
                .collect(),
        )
    }

    /// The standalone bytes of all n servers, ⊥ ones included: the population
    /// [`Self::simulator_node_bytes`] walks, so their ratio is the sharing factor.
    pub fn standalone_bytes(&self) -> Option<u64> {
        Some(self.standalone_all()?.iter().map(|(b, _)| b.total()).sum())
    }

    /// What the simulator itself holds for all n servers' structures (states, logs, checkpoints),
    /// each shared allocation charged once, in the units of [`NodeBytes`]. A shared chunk counts
    /// all its K elements once. Compact certificate state lives beside the engine and is not
    /// included.
    pub fn simulator_node_bytes(&self) -> Option<u64> {
        let mut walk = SharedWalk::default();
        match &self.nodes {
            SmrNodes::Ext(_) => return None,
            SmrNodes::Comp(nodes) => {
                for nd in nodes {
                    walk.state(nd.shared_state());
                    walk.log(nd.log_perm_arc());
                }
            }
            SmrNodes::Rec(nodes) => {
                let clients = self.tracked_clients();
                for nd in nodes {
                    walk.state(nd.shared_state());
                    walk.log(nd.log_perm_arc());
                    let cp = nd.checkpoint();
                    if walk.cps.insert(std::ptr::from_ref(cp) as usize) {
                        walk.state(&cp.s);
                        walk.bytes += pre_bytes(cp);
                        walk.bytes += cp
                            .certs
                            .as_ref()
                            .map_or(0, |c| cert_state_bytes(c, &clients));
                    }
                }
            }
        }
        Some(walk.bytes)
    }

    /// Observer and tracker rows, every engine but extended.
    fn observer_rows(&self, add: &mut impl FnMut(&'static str, u64)) {
        add("observer.canonical", self.checker.canonical().heap_bytes());
        add("observer.exec_dup", self.exec_dup.heap_bytes());
        add("observer.retired_samples", self.retired.heap_bytes());
        add(
            "observer.metrics_rows",
            (self.metrics.capacity() * size_of::<SmrRoundMetrics>()) as u64,
        );
        match &self.rec {
            Some(rec) => {
                add("observer.rec_rows", rec.heap_bytes_rows());
                add("observer.indexes", rec.heap_bytes_indexes());
                add("observer.lineage", rec.heap_bytes_lineage());
            }
            None => add("observer.indexes", self.canon_index.heap_bytes()),
        }
        for t in &self.trackers {
            add(
                "tracker.spread",
                (t.spread.capacity() * size_of::<SpreadPoint>()) as u64,
            );
            add(
                "tracker.attempts",
                (t.attempts.capacity() * size_of::<DeliveryAttempt>()) as u64,
            );
            add(
                "tracker.amp_receivers",
                (t.amp_receivers.capacity() * size_of::<u32>()) as u64,
            );
        }
    }

    /// Retained bytes by category: the full walk for recovery, observer and tracker rows only
    /// for compact.
    pub fn memory_accounting(&self) -> Option<Accounting> {
        let nodes = match &self.nodes {
            SmrNodes::Rec(nodes) => nodes,
            SmrNodes::Comp(nodes) => {
                let mut acc = Accounting {
                    round: self.round,
                    nodes: nodes.len(),
                    ..Default::default()
                };
                self.observer_rows(&mut |key, bytes| *acc.rows.entry(key).or_default() += bytes);
                return Some(acc);
            }
            SmrNodes::Ext(_) => return None,
        };
        let mut acc = Accounting {
            round: self.round,
            nodes: nodes.len(),
            walked_nodes: true,
            ..Default::default()
        };
        let mut add = |key: &'static str, bytes: u64| {
            *acc.rows.entry(key).or_default() += bytes;
        };

        let clients = self.tracked_clients();
        let mut seen_logs = std::collections::HashSet::new();
        let mut seen_chunks = std::collections::HashSet::new();
        // One seen-set per chunk kind: a shared set lets one walk suppress the other's charge.
        let mut seen_state_chunks = std::collections::HashSet::new();
        let mut seen_cp_state_chunks = std::collections::HashSet::new();
        let mut distinct_state_chunks = 0usize;
        let mut seen_cps = std::collections::HashSet::new();
        let mut seen_states = std::collections::HashSet::new();
        let mut distinct_node_states = 0usize;
        let mut distinct_cp_only_states = 0usize;
        let (mut distinct_logs, mut distinct_cps) = (0usize, 0usize);
        let mut distinct_chunks = 0usize;
        let mut stored_in_one_window = 0usize;
        let (mut log_len_bytes, mut perm_len_bytes) = (0u64, 0u64);

        // Node states first, so the node/checkpoint-only split does not depend on walk order.
        for nd in nodes.iter() {
            let state = nd.shared_state();
            let (executed, sn) = state_bytes(state);
            add("node.state.executed", executed);
            add("node.state.sn", sn);
            if seen_states.insert(std::ptr::from_ref(state) as usize) {
                distinct_node_states += 1;
                add("node.state.sn.distinct", sn);
                for ptr in state.executed_chunk_ptrs() {
                    if seen_state_chunks.insert(ptr as usize) {
                        distinct_state_chunks += 1;
                        add(
                            "node.state.executed.distinct",
                            (protocol::shared_state::ExecSeq::K * size_of::<Entry>()) as u64,
                        );
                    }
                }
                let owned_tail = state.retained_len()
                    - state.executed_sealed_count() * protocol::shared_state::ExecSeq::K;
                add(
                    "node.state.executed.distinct",
                    (owned_tail * size_of::<Entry>()) as u64,
                );
            }

            if let Some(l) = nd.log_perm_arc()
                && seen_logs.insert(std::sync::Arc::as_ptr(l.entries()) as usize)
            {
                let (log, perm) = (l.entries(), l.perm());
                distinct_logs += 1;
                add(
                    "node.log.entries",
                    (log.retained_len() * size_of::<Timed>()) as u64,
                );
                add("node.log.perm", (perm.capacity() * size_of::<u32>()) as u64);
                log_len_bytes += (log.len() * size_of::<Timed>()) as u64;
                perm_len_bytes += (perm.len() * size_of::<u32>()) as u64;

                for ptr in log.chunk_ptrs() {
                    if seen_chunks.insert(ptr as usize) {
                        distinct_chunks += 1;
                        add(
                            "node.log.entries.distinct",
                            (protocol::compact::LogSeq::K * size_of::<Timed>()) as u64,
                        );
                    }
                }
                let owned_tail =
                    log.retained_len() - log.sealed_count() * protocol::compact::LogSeq::K;
                add(
                    "node.log.entries.distinct",
                    (owned_tail * size_of::<Timed>()) as u64,
                );
            }
        }

        for nd in nodes.iter() {
            let cp = nd.checkpoint_shared();
            if seen_cps.insert(std::sync::Arc::as_ptr(&cp) as usize) {
                distinct_cps += 1;
                let (executed, sn) = state_bytes(&cp.s);
                add("checkpoint.state.executed", executed);
                add("checkpoint.state.sn", sn);
                if seen_states.insert(std::sync::Arc::as_ptr(&cp.s) as usize) {
                    distinct_cp_only_states += 1;
                    add("checkpoint.state.sn.distinct", sn);
                    for ptr in cp.s.executed_chunk_ptrs() {
                        if seen_cp_state_chunks.insert(ptr as usize) {
                            add(
                                "checkpoint.state.executed.distinct",
                                (protocol::shared_state::ExecSeq::K * size_of::<Entry>()) as u64,
                            );
                        }
                    }
                    let owned_tail = cp.s.retained_len()
                        - cp.s.executed_sealed_count() * protocol::shared_state::ExecSeq::K;
                    add(
                        "checkpoint.state.executed.distinct",
                        (owned_tail * size_of::<Entry>()) as u64,
                    );
                }
                if let Some(pre) = &cp.p {
                    add(
                        "checkpoint.pre",
                        (pre.capacity() * size_of::<Timed>()) as u64,
                    );
                }
                let (forest, windows, stored) = cert_bytes(cp.certs.as_ref(), &clients);
                add("checkpoint.certs.forest", forest);
                add("checkpoint.certs.windows", windows);
                stored_in_one_window = stored_in_one_window.max(stored);
            }
        }
        acc.distinct_logs = distinct_logs;
        acc.distinct_log_chunks = distinct_chunks;
        acc.distinct_state_chunks = distinct_state_chunks;
        acc.distinct_node_states = distinct_node_states;
        acc.distinct_states = distinct_node_states + distinct_cp_only_states;
        acc.distinct_checkpoints = distinct_cps;
        acc.distinct_clients = clients.len();
        acc.cert_stored_commands = stored_in_one_window;
        acc.log_len_bytes = log_len_bytes;
        acc.perm_len_bytes = perm_len_bytes;

        self.observer_rows(&mut add);

        let n = nodes.len() as u64;
        let k = self.cfg.k as u64;
        acc.transient_per_round = n * k * (size_of::<protocol::recovery::RecoveryReply>() as u64)
            + n * k * size_of::<u32>() as u64
            + n * (size_of::<Option<protocol::log::Log>>() as u64)
            + n * (size_of::<u64>() as u64 + size_of::<bool>() as u64 * 2);
        Some(acc)
    }
}
