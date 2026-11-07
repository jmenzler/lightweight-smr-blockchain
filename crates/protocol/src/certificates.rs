//! §5 certificates (Theorem 6): Merkle forest roots and per-client chains; SHA-256 with RFC 6962 domain separation.

use crate::compact::Entry;
use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];

pub fn leaf_hash(entry: &Entry) -> Hash {
    // DETERMINIZED: [DET-8] §5 asks only for a collision-resistant hash; SHA-256, RFC 6962 prefixes
    let mut h = Sha256::new();
    h.update([0x00]);
    match entry {
        Entry::Cmd(c) => {
            h.update([0x01]);
            h.update(c.client.to_le_bytes());
            h.update(c.sn.to_le_bytes());
            h.update(c.op.to_le_bytes());
        }
        Entry::Null { client, sn } => {
            h.update([0x02]);
            h.update(client.to_le_bytes());
            h.update(sn.to_le_bytes());
        }
        Entry::Nop(op) => {
            h.update([0x03]);
            h.update(op.to_le_bytes());
        }
    }
    h.finalize().into()
}

fn interior_hash(left: &Hash, right: &Hash) -> Hash {
    let mut h = Sha256::new();
    h.update([0x01]);
    h.update(left);
    h.update(right);
    h.finalize().into()
}

fn fold(leaf: Hash, pos: u64, chain: &[Hash]) -> Hash {
    let mut h = leaf;
    for (i, sib) in chain.iter().enumerate() {
        h = if (pos >> i) & 1 == 1 {
            interior_hash(sib, &h)
        } else {
            interior_hash(&h, sib)
        };
    }
    h
}

fn lca_child_level(a: u64, b: u64) -> usize {
    assert_ne!(a, b, "the lca is defined for two distinct leaves");
    (63 - (a ^ b).leading_zeros()) as usize
}

/// Merkle Mountain Range over the committed sequence, storing peaks only (§5 pp. 27–28).
#[derive(Debug, Clone)]
pub struct MmrForest {
    peaks: Vec<(u32, Hash, u64)>,
    m: u64,
}

impl MmrForest {
    pub fn new() -> Self {
        Self {
            peaks: Vec::new(),
            m: 0,
        }
    }

    pub fn append(&mut self, leaf: Hash) -> Vec<MergeEvent> {
        self.peaks.push((0, leaf, self.m));
        self.m += 1;
        let mut events = Vec::new();
        while self.peaks.len() >= 2 {
            let [(lh, left, lstart), (rh, right, _)] = self.peaks[self.peaks.len() - 2..] else {
                unreachable!()
            };
            if lh != rh {
                break;
            }
            let parent = interior_hash(&left, &right);
            events.push(MergeEvent {
                height: lh,
                left,
                right,
                start: lstart,
                parent,
            });
            self.peaks.truncate(self.peaks.len() - 2);
            self.peaks.push((lh + 1, parent, lstart));
        }
        events
    }

    pub fn len(&self) -> u64 {
        self.m
    }

    pub fn is_empty(&self) -> bool {
        self.m == 0
    }

    /// Peak heights, descending (= binary decomposition of m).
    pub fn peak_heights(&self) -> Vec<u32> {
        self.peaks.iter().map(|&(h, _, _)| h).collect()
    }

    /// Root hashes h₁,…,h_a, leftmost (tallest) tree first.
    pub fn roots(&self) -> Vec<Hash> {
        self.peaks.iter().map(|&(_, h, _)| h).collect()
    }

    /// (height, root, start) per peak, tallest first — the full stored state.
    pub fn peaks(&self) -> Vec<(u32, Hash, u64)> {
        self.peaks.clone()
    }

    /// Whether both forests can be prefixes of one sequence, judged on the leading peaks that
    /// cover the same leaves; later peaks cannot be compared without the leaves.
    pub fn consistent_with(&self, other: &MmrForest) -> bool {
        self.peaks
            .iter()
            .zip(&other.peaks)
            .take_while(|((lh, ..), (rh, ..))| lh == rh)
            .all(|((_, left, _), (_, right, _))| left == right)
    }

    fn covering_peak(&self, pos: u64) -> Option<(u32, Hash)> {
        self.peaks
            .iter()
            .find(|&&(h, _, start)| pos >= start && pos < start + (1u64 << h))
            .map(|&(h, root, _)| (h, root))
    }
}

impl Default for MmrForest {
    fn default() -> Self {
        Self::new()
    }
}

/// Two equal-height peaks merged; the parent covers [start, start + 2^(height+1)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeEvent {
    pub height: u32,
    pub left: Hash,
    pub right: Hash,
    pub start: u64,
    pub parent: Hash,
}

use crate::compact::ClientId;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCommand {
    pub leaf: Hash,
    pub pos: u64,
    pub sn: u64,
    pub chain: Vec<Hash>,
}

impl StoredCommand {
    fn extend_by(&mut self, event: &MergeEvent) {
        let span = event.height + 1;
        if self.pos >> span != event.start >> span {
            return;
        }
        assert_eq!(self.chain.len(), event.height as usize);
        if (self.pos >> event.height) & 1 == 0 {
            self.chain.push(event.right);
        } else {
            self.chain.push(event.left);
        }
    }
}

/// What a useful server stores (§5 pp. 27–28): forest roots plus each client's last two chains.
#[derive(Debug, Clone)]
pub struct ServerCertState {
    forest: MmrForest,
    windows: BTreeMap<ClientId, (Option<StoredCommand>, StoredCommand)>,
}

/// Ack payload for the client's newest command: hc(x₁) and p(x₁) of the previous one (§5 p. 28).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub prev_chain: Vec<Hash>,
    pub prev_pos: u64,
    // DEVIATION: [DEV-4] the payload also names sn(x_1); the client files the chain under it
    pub prev_sn: u64,
}

impl ServerCertState {
    pub fn new() -> Self {
        Self {
            forest: MmrForest::new(),
            windows: BTreeMap::new(),
        }
    }

    pub fn append(&mut self, entry: &Entry) -> Vec<MergeEvent> {
        let leaf = leaf_hash(entry);
        let pos = self.forest.len();
        // DETERMINIZED: [DET-19] every committed entry with a (client, sn), ⊥ included, rotates the window
        if let Some((client, sn)) = entry.client_sn() {
            let stored = StoredCommand {
                leaf,
                pos,
                sn,
                chain: Vec::new(),
            };
            match self.windows.get_mut(&client) {
                Some((prev, last)) => {
                    *prev = Some(std::mem::replace(last, stored));
                }
                None => {
                    self.windows.insert(client, (None, stored));
                }
            }
        }
        let events = self.forest.append(leaf);
        for event in &events {
            for (prev, last) in self.windows.values_mut() {
                for sc in prev.iter_mut().chain([last]) {
                    sc.extend_by(event);
                }
            }
        }
        events
    }

    /// The stored (x₁, x₂) window of a client: (previous, newest).
    pub fn last_two(&self, client: ClientId) -> Option<(Option<&StoredCommand>, &StoredCommand)> {
        let (prev, last) = self.windows.get(&client)?;
        Some((prev.as_ref(), last))
    }

    pub fn forest(&self) -> &MmrForest {
        &self.forest
    }

    /// Theorem 6 verification of a client's certificate.
    pub fn verify(&self, client: ClientId, cert: &Certificate) -> bool {
        // DEVIATION: [DEV-2] stricter than §5: the entry must belong to `client`
        let entry = match cert {
            Certificate::Newest { entry } | Certificate::Chained { entry, .. } => entry,
        };
        if entry.client_sn().map(|(c, _)| c) != Some(client) {
            return false;
        }
        match cert {
            Certificate::Newest { entry } => {
                let Some((prev, last)) = self.last_two(client) else {
                    return false;
                };
                let leaf = leaf_hash(entry);
                last.leaf == leaf || prev.is_some_and(|p| p.leaf == leaf)
            }
            Certificate::Chained { entry, pos, chain } => {
                if *pos >= self.forest.len() {
                    return false;
                }
                // Lemma 5.1 (p. 29): fold a chain prefix, never the whole chain.
                let leaf = leaf_hash(entry);
                // Case (a): lc(lca) is a tree root.
                if let Some((height, root)) = self.forest.covering_peak(*pos)
                    && chain.len() >= height as usize
                    && fold(leaf, *pos, &chain[..height as usize]) == root
                {
                    return true;
                }
                // DETERMINIZED: [DET-18] the certificate does not name x_{k+1}; both window records are tried
                // Case (b): lca exists; x_{k+1} may sit in either window slot (p. 29).
                let Some((prev, last)) = self.last_two(client) else {
                    return false;
                };
                prev.into_iter().chain([last]).any(|w| {
                    if *pos == w.pos {
                        return leaf == w.leaf;
                    }
                    if *pos > w.pos {
                        return false;
                    }
                    let t = lca_child_level(*pos, w.pos);
                    chain.len() >= t
                        && w.chain.len() > t
                        && (w.pos >> t) & 1 == 1
                        && pos >> t == (w.pos >> t) ^ 1
                        && w.chain[t] == fold(leaf, *pos, &chain[..t])
                })
            }
        }
    }

    /// The hc(x₁)/p(x₁) ack payload; None until the client has two committed commands.
    pub fn notification(&self, client: ClientId) -> Option<Notification> {
        let (prev, _) = self.windows.get(&client)?;
        let prev = prev.as_ref()?;
        Some(Notification {
            prev_chain: prev.chain.clone(),
            prev_pos: prev.pos,
            prev_sn: prev.sn,
        })
    }
}

impl Default for ServerCertState {
    fn default() -> Self {
        Self::new()
    }
}

/// What client c stores (§5 p. 28): received chains and positions plus its issued commands.
pub struct ClientChains {
    client: ClientId,
    issued: BTreeMap<u64, Entry>,
    received: BTreeMap<u64, (u64, Vec<Hash>)>,
    acked_sn: u64,
}

impl ClientChains {
    pub fn new(client: ClientId) -> Self {
        Self {
            client,
            issued: BTreeMap::new(),
            received: BTreeMap::new(),
            acked_sn: 0,
        }
    }

    pub fn client(&self) -> ClientId {
        self.client
    }

    pub fn record_issue(&mut self, entry: &Entry) {
        let (client, sn) = entry.client_sn().expect("clients issue client commands");
        assert_eq!(client, self.client);
        self.issued.insert(sn, entry.clone());
    }

    /// Record a commit ack, keeping the longer chain per command (§5 p. 28).
    pub fn on_commit_ack(&mut self, acked_sn: u64, note: Option<Notification>) {
        self.acked_sn = self.acked_sn.max(acked_sn);
        let Some(note) = note else { return };
        match self.received.get_mut(&note.prev_sn) {
            Some((_, chain)) if chain.len() >= note.prev_chain.len() => {}
            _ => {
                self.received
                    .insert(note.prev_sn, (note.prev_pos, note.prev_chain));
            }
        }
    }

    /// Next sequence number: sn = k + 2 once k ≥ 1 chains were received (§5 p. 28).
    pub fn next_sn(&self) -> u64 {
        self.acked_sn + 1
    }

    pub fn chain_for(&self, sn: u64) -> Option<(u64, &[Hash])> {
        let (pos, chain) = self.received.get(&sn)?;
        Some((*pos, chain))
    }

    pub fn issued_count(&self) -> u64 {
        self.issued.len() as u64
    }

    /// The certificate (x, p(x), h̄c(x)) for `sn`, stitched per Lemma 5.1 (§5 p. 28).
    pub fn build_certificate(&self, sn: u64) -> Option<Certificate> {
        let entry = self.issued.get(&sn)?.clone();
        if sn == self.acked_sn && !self.received.contains_key(&sn) {
            return Some(Certificate::Newest { entry });
        }
        // DETERMINIZED: [DET-20] hc(x_j) is extended from later received chains only
        let (pos, base) = self.received.get(&sn)?;
        let mut chain = base.clone();
        for (later_sn, (later_pos, later_chain)) in self.received.range(sn + 1..) {
            if later_pos == pos {
                continue;
            }
            let s = lca_child_level(*pos, *later_pos);
            if chain.len() == s && later_chain.len() >= s {
                let Some(later_entry) = self.issued.get(later_sn) else {
                    continue;
                };
                chain.push(fold(leaf_hash(later_entry), *later_pos, &later_chain[..s]));
            }
            if chain.len() > s && later_chain.len() > chain.len() {
                chain.extend_from_slice(&later_chain[chain.len()..]);
            }
        }
        Some(Certificate::Chained {
            entry,
            pos: *pos,
            chain,
        })
    }
}

/// The certificate a client submits for verification (§5 p. 28).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Certificate {
    Chained {
        entry: Entry,
        pos: u64,
        chain: Vec<Hash>,
    },
    /// x = x_{k+1}: no position or chain known yet.
    Newest { entry: Entry },
}
