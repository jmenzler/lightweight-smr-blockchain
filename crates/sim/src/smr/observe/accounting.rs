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

impl SmrState {
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

        let clients: Vec<u32> = {
            let mut v: Vec<u32> = self.trackers.iter().map(|t| t.client).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
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
