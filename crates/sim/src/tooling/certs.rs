//! §5 certificate driver harness: per-server cert state, commit acks, and Thm 6 verification.

use protocol::certificates::{Certificate, ClientChains, MergeEvent, MmrForest, ServerCertState};
use protocol::compact::{ClientCommand, Entry};
use protocol::recovery::Checkpoint;

use crate::smr::{AttemptOutcome, SmrState};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct CertHarness {
    servers: ServerCerts,
    fed: Vec<usize>,
    bottom: Vec<bool>,
    clients: BTreeMap<u32, ClientChains>,
    sn_by_op: BTreeMap<u64, (u32, u64)>,
    last_merges: Vec<(usize, MergeEvent)>,
}

enum ServerCerts {
    /// Built by appending each server's executed growth (compact, extended).
    Replayed(Vec<ServerCertState>),
    /// Read through each recovery server's checkpoint; a node replaces its checkpoint, never
    /// mutates it, so a held one keeps the certs it was observed with.
    Checkpointed(Vec<Arc<Checkpoint>>),
}

impl ServerCerts {
    fn get(&self, i: usize) -> &ServerCertState {
        match self {
            Self::Replayed(states) => &states[i],
            Self::Checkpointed(checkpoints) => checkpoints[i]
                .certs
                .as_ref()
                .expect("a checkpointed harness holds cert-carrying checkpoints"),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Replayed(states) => states.len(),
            Self::Checkpointed(checkpoints) => checkpoints.len(),
        }
    }
}

/// Verification tally: `covered` useful servers (Thm 6, p. 28: not blocked, L_i ≠ ⊥) are expected
/// to verify the certificate, `accepted` of those did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyTally {
    pub covered: usize,
    pub accepted: usize,
}

impl CertHarness {
    pub fn new(n: usize) -> Self {
        Self {
            servers: ServerCerts::Replayed((0..n).map(|_| ServerCertState::new()).collect()),
            fed: vec![0; n],
            bottom: vec![false; n],
            clients: BTreeMap::new(),
            sn_by_op: BTreeMap::new(),
            last_merges: Vec::new(),
        }
    }

    pub fn register_client(&mut self, client: u32) {
        self.clients
            .entry(client)
            .or_insert_with(|| ClientChains::new(client));
    }

    pub fn issued_count(&self, client: u32) -> u64 {
        self.clients.get(&client).map_or(0, |c| c.issued_count())
    }

    pub fn record_issue(&mut self, client: u32, sn: u64, op: u64) {
        self.sn_by_op.insert(op, (client, sn));
        self.clients
            .get_mut(&client)
            .expect("client registered")
            .record_issue(&Entry::Cmd(ClientCommand { client, sn, op }));
    }

    pub fn acked_sn(&self, client: u32) -> u64 {
        self.clients[&client].next_sn() - 1
    }

    pub fn client(&self, client: u32) -> &ClientChains {
        &self.clients[&client]
    }

    /// Deliver the round's acks from the start-of-round forests, then feed this round's executed
    /// growth; call once after every step.
    pub fn observe_round(&mut self, state: &SmrState) {
        self.deliver_acks(state);
        self.last_merges.clear();
        self.bottom = state.bottom_logs();
        let ServerCerts::Replayed(servers) = &mut self.servers else {
            panic!("observe_round on a harness fed by observe_round_rec");
        };
        let executed = state.executed_seqs();
        for (i, seq) in executed.iter().enumerate() {
            if seq.len() < self.fed[i] {
                // Cert state is a pure function of the executed sequence, so a rewritten sequence rebuilds by replay.
                servers[i] = ServerCertState::new();
                self.fed[i] = 0;
            }
            for entry in &seq[self.fed[i]..] {
                let events = servers[i].append(entry);
                self.last_merges.extend(events.into_iter().map(|e| (i, e)));
            }
            self.fed[i] = seq.len();
        }
    }

    /// As [`Self::observe_round`]: acks read the start-of-round checkpoint certs, then the
    /// end-of-round ones are installed.
    pub fn observe_round_rec(&mut self, state: &SmrState) {
        self.deliver_acks(state);
        self.last_merges.clear();
        self.bottom = state.bottom_logs();
        let checkpoints = (0..self.servers.len())
            .map(|i| {
                state.checkpoint_certs(i).expect(
                    "recovery cert state is spec-gated; set certs: true in the scenario \
                     (or build the state with SmrState::new_with_certs)",
                );
                state
                    .recovery_node(i)
                    .expect("a checkpoint implies a recovery node")
                    .checkpoint_shared()
            })
            .collect();
        self.servers = ServerCerts::Checkpointed(checkpoints);
    }

    // §5 p. 28: the ack comes from the one server whose triage acked, with that server's chain at
    // reply time; Alg 5 replies before the round's commit, so the forests must not yet hold it.
    fn deliver_acks(&mut self, state: &SmrState) {
        let round = state.round();
        let report = state.report();
        for c in &report.commands {
            let Some(&(client, sn)) = self.sn_by_op.get(&c.op) else {
                continue;
            };
            if c.committed_ack_round != Some(round) {
                continue;
            }
            // Reads attempts at the ack round itself; a lean+certs pairing panics here by design.
            let acker = c
                .attempts
                .iter()
                .find(|a| a.round == round && a.outcome == AttemptOutcome::AckCommitted)
                .map(|a| a.target as usize)
                .expect("ack round implies an AckCommitted attempt");
            let note = self.servers.get(acker).notification(client);
            self.clients
                .get_mut(&client)
                .expect("client registered")
                .on_commit_ack(sn, note);
        }
    }

    pub fn merges_last_round(&self) -> &[(usize, MergeEvent)] {
        &self.last_merges
    }

    pub fn servers(&self) -> impl Iterator<Item = &ServerCertState> {
        (0..self.servers.len()).map(|i| self.servers.get(i))
    }

    pub fn clients_iter(&self) -> impl Iterator<Item = &ClientChains> {
        self.clients.values()
    }

    /// Servers of equal length hold equal roots; servers of different length agree on their shared
    /// leading peaks (`MmrForest::consistent_with`).
    pub fn roots_consistent(&self) -> bool {
        let mut by_len: BTreeMap<u64, &MmrForest> = BTreeMap::new();
        for server in self.servers() {
            let forest = server.forest();
            match by_len.get(&forest.len()) {
                Some(rep) if rep.roots() != forest.roots() => return false,
                Some(_) => {}
                None => {
                    by_len.insert(forest.len(), forest);
                }
            }
        }
        let reps: Vec<&MmrForest> = by_len.into_values().collect();
        reps.iter()
            .enumerate()
            .all(|(i, a)| reps[i + 1..].iter().all(|b| a.consistent_with(b)))
    }

    pub fn verify_everywhere(&self, client: u32, cert: &Certificate, mask: &[bool]) -> VerifyTally {
        let mut tally = VerifyTally {
            covered: 0,
            accepted: 0,
        };
        for (i, s) in self.servers().enumerate() {
            if mask[i] || self.bottom[i] {
                continue;
            }
            tally.covered += 1;
            tally.accepted += usize::from(s.verify(client, cert));
        }
        tally
    }
}
