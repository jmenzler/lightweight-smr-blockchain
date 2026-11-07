//! Plain reference implementations of one server round of Algorithm 5 (compact
//! (k,ℓ)-median rule, p. 26) and Algorithm 6 (recovery protocol, p. 31), one
//! function per box step, over plain `Vec`s: no chunks, no index, no sharing.
//!
//! Where a box leaves a choice open, the reference makes the production code's
//! choice and says so with a `DETERMINIZED:` tag; the differential tests in
//! `differential.rs` then demand exact equality with the production nodes after
//! every round. Because those choices (and `Entry`/`Timed` ordering, `sample`) are shared,
//! agreement guards refactorings of the production code, not the readings themselves; the
//! readings are pinned by the hand-derived tests next to each box.

use protocol::compact::{ClientCommand, ClientId, Entry, Timed, Triage};
use protocol::merge::{StampTie, UnionStamp};
use protocol::recovery::RState;
use rand::Rng;
use rand::seq::index::sample;
use std::collections::{BTreeMap, BTreeSet};

/// S_i: the executed commands in commit order, plus sn(c) for every client.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub executed: Vec<Entry>,
    pub sn: BTreeMap<ClientId, u64>,
}

impl State {
    /// A command extends S and sets sn(c); ⊥ only sets sn(c); x₀/x_d change nothing.
    fn execute(&mut self, entry: &Entry) {
        match entry {
            Entry::Cmd(x) => {
                self.executed.push(entry.clone());
                self.sn.insert(x.client, x.sn);
            }
            Entry::Null { client, sn } => {
                self.sn.insert(*client, *sn);
            }
            Entry::Nop(_) => {}
        }
    }
}

/// The seed command x₀ every log starts with.
fn x0() -> Timed {
    Timed {
        entry: Entry::Nop(0),
        round: 0,
    }
}

/// The dummy command x_d appended to an emptied log.
fn xd(round: u64) -> Timed {
    // DETERMINIZED: x_d is `Nop(round)` stamped `round` (the box only asks for a no-op).
    Timed {
        entry: Entry::Nop(round),
        round,
    }
}

/// Length of the largest prefix whose commands all have age ≥ `t`.
/// Alg 6 (p. 31): P is a "sequence of pre-committed commands", so it prefixes L_i by command.
pub fn is_command_prefix(p: &[Timed], log: &[Timed]) -> bool {
    log.len() >= p.len() && p.iter().zip(log).all(|(p, l)| p.entry == l.entry)
}

fn aged_prefix(log: &[Timed], round: u64, t: u64) -> usize {
    log.iter()
        .take_while(|x| round.saturating_sub(x.round) >= t)
        .count()
}

/// "Chooses a subset M of ℓ of the replies uniformly at random"; `None` below ℓ.
pub fn choose_m<R: Rng + ?Sized>(replies: usize, ell: usize, rng: &mut R) -> Option<Vec<usize>> {
    // DETERMINIZED: M is `sample(rng, replies, ℓ)` in draw order (the simulator's RNG contract).
    (replies >= ell).then(|| sample(rng, replies, ell).into_vec())
}

/// How the merge reads a log's round stamps: the comparator key and which sighting's stamp L̄ keeps.
#[derive(Debug, Clone, Copy, Default)]
pub struct MergePolicy {
    pub stamp: StampTie,
    pub union: UnionStamp,
}

/// The extended median rule's merge `L'_i ∘ L̄` over the logs in M (Alg 3, used by Alg 5 step 4 and Alg 6).
pub fn median_merge(m: &[Vec<Timed>], appends: &[(ClientCommand, u64)]) -> Vec<Timed> {
    median_merge_with(m, appends, MergePolicy::default())
}

pub fn median_merge_with(
    m: &[Vec<Timed>],
    appends: &[(ClientCommand, u64)],
    policy: MergePolicy,
) -> Vec<Timed> {
    // DETERMINIZED: median-stamp — logs compare lexicographically as sequences of (command, round),
    // or of commands only with exact ties in draw order.
    let mut sorted: Vec<&Vec<Timed>> = m.iter().collect();
    match policy.stamp {
        StampTie::IncludeRound => sorted.sort(),
        StampTie::CommandsOnly => {
            sorted.sort_by(|a, b| a.iter().map(|t| &t.entry).cmp(b.iter().map(|t| &t.entry)))
        }
    }
    let mut log = sorted[m.len() / 2].clone();
    let median_len = log.len();
    // DETERMINIZED: union-order — L̄ walks the logs in sorted order, then the append requests;
    // containment is keyed on the command, so the first sighting's stamp survives unless the policy
    // re-stamps with the last one. The median L'_i is taken whole, stamps included.
    let requests = appends.iter().map(|(x, round)| Timed {
        entry: Entry::Cmd(*x),
        round: *round,
    });
    for t in sorted
        .iter()
        .flat_map(|l| l.iter().cloned())
        .chain(requests)
    {
        match log.iter().position(|have| have.entry == t.entry) {
            None => log.push(t),
            Some(at) if policy.union == UnionStamp::LastSighting && at >= median_len => {
                log[at].round = t.round;
            }
            Some(_) => {}
        }
    }
    log
}

/// The (client, sn) pairs carrying two different commands in `log`.
pub fn conflicting(log: &[Timed]) -> BTreeSet<(ClientId, u64)> {
    // DETERMINIZED: ⊥ is not "a different command"; only two `Cmd`s with different ops conflict.
    let cmds: Vec<ClientCommand> = log
        .iter()
        .filter_map(|t| match t.entry {
            Entry::Cmd(x) => Some(x),
            _ => None,
        })
        .collect();
    cmds.iter()
        .filter(|a| {
            cmds.iter()
                .any(|b| (a.client, a.sn) == (b.client, b.sn) && a.op != b.op)
        })
        .map(|a| (a.client, a.sn))
        .collect()
}

/// Alg 5 box (p. 26): different commands with the same (client, sn) all become ⊥.
fn replace_duplicates_with_bot(log: &mut [Timed]) {
    let conflicting = conflicting(log);
    for t in log.iter_mut() {
        if let Entry::Cmd(x) = t.entry
            && conflicting.contains(&(x.client, x.sn))
        {
            t.entry = Entry::Null {
                client: x.client,
                sn: x.sn,
            };
        }
    }
}

// ---------------------------------------------------------------- Algorithm 5

/// A reply to a compact-rule request: L_j, plus S_j iff the requester's b was 1.
#[derive(Debug, Clone)]
pub struct CompactReply {
    pub l_j: Vec<Timed>,
    pub s_j: Option<State>,
}

/// Server i under Algorithm 5; `l_i == None` is L_i = ⊥.
#[derive(Debug, Clone)]
pub struct CompactServer {
    pub s_i: State,
    pub l_i: Option<Vec<Timed>>,
    t: u64,
    policy: MergePolicy,
}

impl CompactServer {
    pub fn new(t: u64) -> Self {
        CompactServer {
            s_i: State::default(),
            l_i: Some(vec![x0()]),
            t,
            policy: MergePolicy::default(),
        }
    }

    pub fn with_policy(mut self, policy: MergePolicy) -> Self {
        self.policy = policy;
        self
    }

    fn sn(&self, c: ClientId) -> u64 {
        self.s_i.sn.get(&c).copied().unwrap_or(0)
    }

    // Alg 5: "for every command x received from a client c"
    pub fn on_client_command(&self, x: &ClientCommand) -> Triage {
        let sn_c = self.sn(x.client);
        // Alg 5: "if L_i ≠ ⊥, sn(x) = sn(c) + 1 and x ∉ L_i"
        if let Some(l_i) = &self.l_i
            && x.sn == sn_c + 1
            && !l_i.iter().any(|t| t.entry == Entry::Cmd(*x))
        {
            return Triage::Amplify;
        }
        // Alg 5: "if L_i ≠ ⊥ and sn(x) = sn(c)"
        if self.l_i.is_some() && x.sn == sn_c {
            return Triage::AckCommitted;
        }
        // Alg 5: "otherwise, i ignores x"
        Triage::Ignore
    }

    // Alg 5: "i sends k requests ... and attaches a bit b_i"
    pub fn b_i(&self) -> bool {
        self.l_i.is_none()
    }

    // Alg 5: "if L_i ≠ ⊥ then for any request received by i from some server j"
    pub fn answer(&self, b_j: bool) -> Option<CompactReply> {
        Some(CompactReply {
            l_j: self.l_i.clone()?,
            s_j: b_j.then(|| self.s_i.clone()),
        })
    }

    /// The rest of one round; `m` is the drawn M, `None` iff fewer than ℓ replies.
    pub fn step(
        &mut self,
        replies: &[CompactReply],
        appends: &[(ClientCommand, u64)],
        round: u64,
        m: Option<&[usize]>,
    ) {
        let b_i = self.b_i();
        match m {
            Some(m) => self.at_least_ell_replies(replies, appends, round, m, b_i),
            None => self.fewer_than_ell_replies(replies, round, b_i),
        }
    }

    // Alg 5: "if i receives at least ℓ replies"
    fn at_least_ell_replies(
        &mut self,
        replies: &[CompactReply],
        appends: &[(ClientCommand, u64)],
        round: u64,
        m: &[usize],
        b_i: bool,
    ) {
        // Alg 5: "i chooses a subset M of ℓ of the replies uniformly at random"
        let m: Vec<&CompactReply> = m.iter().map(|&j| &replies[j]).collect();
        // Alg 5: "i sets L_i := L'_i ∘ L̄"
        let logs: Vec<Vec<Timed>> = m.iter().map(|r| r.l_j.clone()).collect();
        let mut l_i = median_merge_with(&logs, appends, self.policy);
        // Alg 5: "if the logs in L_i contain different commands from the same client with the same sequence number"
        replace_duplicates_with_bot(&mut l_i);
        // Alg 5: "if b_i = 1 then i picks any reply S_j"
        // DETERMINIZED: "any reply S_j in M" is the first DRAWN reply that carries a state.
        if b_i && let Some(s_j) = m.iter().find_map(|r| r.s_j.as_ref()) {
            self.s_i = s_j.clone();
        }
        // Alg 5: "i determines the largest prefix P_i of L_i"
        let p_i = aged_prefix(&l_i, round, self.t);
        // Alg 5: "i commits the commands in P_i in the given order on S_i, removes P_i from L_i"
        for x in l_i.drain(..p_i) {
            self.s_i.execute(&x.entry);
        }
        if l_i.is_empty() {
            l_i.push(xd(round));
        }
        self.l_i = Some(l_i);
    }

    // Alg 5: "if i receives less than ℓ replies"
    fn fewer_than_ell_replies(&mut self, replies: &[CompactReply], round: u64, b_i: bool) {
        // Alg 5: "i sets L_i := ⊥"
        self.l_i = None;
        // Alg 5: "if at least one reply is received then i picks any one of them"
        // DETERMINIZED: "picks any one" is the first reply received.
        let Some(r) = replies.first() else {
            return;
        };
        if b_i && let Some(s_j) = &r.s_j {
            self.s_i = s_j.clone();
        }
        for x in &r.l_j[..aged_prefix(&r.l_j, round, self.t)] {
            self.s_i.execute(&x.entry);
        }
    }
}

// ---------------------------------------------------------------- Algorithm 6

/// C_i = (S, P, W); `p == None` is P = ⊥, which is not the empty prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub s: State,
    pub p: Option<Vec<Timed>>,
    pub w: u64,
}

/// A reply from a server with R_j ≠ ⊥: (C_j, R_j), plus L_j for the median rule.
#[derive(Debug, Clone)]
pub struct RecoveryReply {
    pub l_j: Option<Vec<Timed>>,
    pub c_j: Checkpoint,
    pub r_j: RState,
}

/// Server i under Algorithm 6 on top of the extended median rule.
#[derive(Debug, Clone)]
pub struct RecoveryServer {
    pub s_i: State,
    pub l_i: Option<Vec<Timed>>,
    pub r_i: RState,
    pub c_i: Checkpoint,
    ell: usize,
    t: u64,
}

impl RecoveryServer {
    pub fn new(ell: usize, t: u64) -> Self {
        RecoveryServer {
            s_i: State::default(),
            l_i: Some(vec![x0()]),
            r_i: RState::NoReset,
            c_i: Checkpoint {
                s: State::default(),
                p: None,
                w: 0,
            },
            ell,
            t,
        }
    }

    // Alg 3: "for every command x received from a client that is not yet contained in L_i"
    pub fn wants_amplify(&self, x: &ClientCommand) -> bool {
        match &self.l_i {
            None => true,
            Some(l_i) => !l_i.iter().any(|t| t.entry == Entry::Cmd(*x)),
        }
    }

    // Alg 6: "If a request was received from server j and R_i ≠ ⊥"
    // Alg 3: "if L_i ≠ ⊥ then for any log request received by server i from a server j"
    pub fn answer(&self) -> Option<RecoveryReply> {
        (self.r_i != RState::Bot).then(|| RecoveryReply {
            l_j: self.l_i.clone(),
            c_j: self.c_i.clone(),
            r_j: self.r_i,
        })
    }

    /// One round inside a T-window; `m` indexes the log-bearing replies.
    pub fn step(
        &mut self,
        replies: &[RecoveryReply],
        appends: &[(ClientCommand, u64)],
        m: Option<&[usize]>,
    ) {
        self.extended_median_rule(replies, appends, m);
        if replies.len() >= self.ell {
            self.at_least_ell_replies(replies);
        } else {
            self.fewer_than_ell_replies();
        }
    }

    /// Alg 3 over the replies that carry a log.
    fn extended_median_rule(
        &mut self,
        replies: &[RecoveryReply],
        appends: &[(ClientCommand, u64)],
        m: Option<&[usize]>,
    ) {
        let logs: Vec<&Vec<Timed>> = replies.iter().filter_map(|r| r.l_j.as_ref()).collect();
        assert_eq!(
            m.is_some(),
            logs.len() >= self.ell,
            "M is drawn iff ℓ logs arrived"
        );
        self.l_i = match m {
            // Alg 3: "if server i receives at least ℓ replies"
            // Alg 3: "server i chooses a subset M of size ℓ from the received logs"
            // Alg 3: "server i sets L_i := L'_i ∘ L̄"
            Some(m) => {
                let chosen: Vec<Vec<Timed>> = m.iter().map(|&j| logs[j].clone()).collect();
                Some(median_merge(&chosen, appends))
            }
            // Alg 3: "if server i receives less than ℓ logs"
            None => None,
        };
    }

    // Alg 6: "If at least ℓ replies were received"
    fn at_least_ell_replies(&mut self, replies: &[RecoveryReply]) {
        // Alg 6: "If no-reset was received in one reply"
        self.r_i = if replies.iter().any(|x| x.r_j == RState::NoReset) {
            RState::NoReset
        } else {
            RState::Reset
        };
        // Alg 6: "let C' = (S', P', W') be the received checkpoint with largest W'"
        let w_max = replies
            .iter()
            .map(|x| x.c_j.w)
            .max()
            .expect("at least ℓ replies");
        // DETERMINIZED: latest-checkpoint — "ties broken arbitrarily" takes the LAST reply at W'.
        let c_prime = &replies.iter().rev().find(|x| x.c_j.w == w_max).unwrap().c_j;
        // Alg 6: "If W' > W"
        if c_prime.w > self.c_i.w {
            self.s_i = c_prime.s.clone();
            self.l_i = c_prime.p.clone();
            self.c_i = c_prime.clone();
        }
    }

    // Alg 6: "If less than ℓ replies were received"
    fn fewer_than_ell_replies(&mut self) {
        self.r_i = RState::Bot;
    }

    /// Between T-windows: the three bullets run in order, each on the result of the last.
    pub fn end_window(&mut self, next_window: u64, round: u64) {
        // Alg 6: "If R_i = reset"
        if self.r_i == RState::Reset {
            self.s_i = self.c_i.s.clone();
            self.l_i = self.c_i.p.clone();
        }
        // Alg 6: "If L_i ≠ ⊥"
        if let Some(l_i) = &mut self.l_i {
            // Alg 6: "for current checkpoint C_i = (S, P, W), commit all commands in P on S_i and remove the prefix P from L_i"
            // P = ⊥ (the genesis checkpoint) commits nothing.
            let p = self.c_i.p.clone().unwrap_or_default();
            assert!(is_command_prefix(&p, l_i), "P must be a prefix of L_i");
            for x in l_i.drain(..p.len()) {
                self.s_i.execute(&x.entry);
            }
            if l_i.is_empty() {
                l_i.push(xd(round));
            }
            // Alg 6: "make new checkpoint C_i := (S_i, P_i, W')"
            let p_i = l_i[..aged_prefix(l_i, round, self.t)].to_vec();
            self.c_i = Checkpoint {
                s: self.s_i.clone(),
                p: Some(p_i),
                w: next_window,
            };
            // Alg 6: "set R_i := no-reset"
            self.r_i = RState::NoReset;
        }
        // Alg 6: "If L_i = ⊥"
        if self.l_i.is_none() {
            self.r_i = RState::Reset;
        }
    }
}
