//! Production nodes against the plain reference implementations in `reference.rs`,
//! side by side over seeded multi-round worlds. After EVERY round (and every
//! window boundary) each node's log, shared state (executed sequence and
//! sequence numbers), reset state and checkpoint must equal the reference's, and
//! the production log's carried sort index must still sort its entries.
//!
//! Replies come from the production nodes' own `answer`, so the production merge sees
//! logs that share `Arc` chunks exactly as it does in the simulator.
//!
//! The reference makes the production code's choice wherever the paper leaves one open, so
//! agreement is a refactoring guard (chunking, indexing, sharing), not independent evidence
//! for those choices; the hand-derived tests next to each box pin them. Each world
//! counts how often the cases the box is most likely to get wrong occur, and
//! fails if a case stops being reached.

use crate::reference::{self as spec, CompactServer, MergePolicy, RecoveryServer};
use protocol::Config;
use protocol::chunked::ChunkSeq;
use protocol::compact::{ClientCommand, CompactNode, CompactReply, SharedState, Timed, Triage};
use protocol::log::Log;
use protocol::merge::{StampTie, UnionStamp};
use protocol::recovery::{Checkpoint, RState, RecoveryNode, RecoveryReply};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;
use std::sync::Arc;

const SEEDS: u64 = 200;
const ROUNDS: u64 = 60;
const N: usize = 16;
const SIGMA: usize = 3;

fn config(seed: u64) -> Config {
    if seed % 3 == 2 {
        Config::new(8, 5).unwrap()
    } else {
        Config::default()
    }
}

fn state_of(s: &SharedState) -> spec::State {
    spec::State {
        executed: s.untruncated().to_vec(),
        sn: s.sn_iter().collect(),
    }
}

fn checkpoint_of(c: &Checkpoint) -> spec::Checkpoint {
    spec::Checkpoint {
        s: state_of(&c.s),
        p: c.p.clone(),
        w: c.w,
    }
}

/// Blocking in phases of random length, each with its own blocked fraction.
struct Surges {
    betas: &'static [f64],
    beta: f64,
    phase_left: u64,
}

impl Surges {
    fn new(betas: &'static [f64]) -> Self {
        Surges {
            betas,
            beta: 0.0,
            phase_left: 0,
        }
    }

    fn mask(&mut self, rng: &mut ChaCha12Rng) -> Vec<bool> {
        if self.phase_left == 0 {
            self.beta = self.betas[rng.random_range(0..self.betas.len())];
            self.phase_left = rng.random_range(2..12);
        }
        self.phase_left -= 1;
        (0..N).map(|_| rng.random_bool(self.beta)).collect()
    }
}

/// Any two drawn logs holding a common sealed chunk by the same allocation.
fn shares_chunks(logs: &[&ChunkSeq<Timed>]) -> bool {
    logs.iter()
        .enumerate()
        .any(|(a, x)| logs[a + 1..].iter().any(|y| x.shared_prefix_chunks(y) > 0))
}

fn appends_for(
    delivered: &[ClientCommand],
    mask: &[bool],
    round: u64,
    rng: &mut ChaCha12Rng,
) -> Vec<Vec<(ClientCommand, u64)>> {
    let mut appends = vec![Vec::new(); N];
    for x in delivered {
        for _ in 0..SIGMA {
            let j = rng.random_range(0..N);
            if !mask[j] {
                appends[j].push((*x, round));
            }
        }
    }
    appends
}

#[derive(Debug, Default)]
struct CompactCoverage {
    below_ell_with_reply: u64,
    below_ell_state_adopted: u64,
    step4_state_adopted: u64,
    nulled_duplicate: u64,
    shared_chunks: u64,
    executed_commands: u64,
    drawn_logs_with_repeated_entries: u64,
}

/// An equivocating client's second command for the same sequence number.
const EQUIVOCATE: u64 = 1 << 32;

fn compact_world(seed: u64, policy: MergePolicy, cov: &mut CompactCoverage) {
    let cfg = config(seed);
    let t_commit = 8;
    let mut rng = ChaCha12Rng::seed_from_u64(seed);
    let mut prod: Vec<CompactNode> = (0..N)
        .map(|_| CompactNode::new(cfg, t_commit).with_merge_policy(policy.stamp, policy.union))
        .collect();
    let mut refs: Vec<CompactServer> = (0..N)
        .map(|_| CompactServer::new(t_commit).with_policy(policy))
        .collect();
    let mut clients: Vec<ClientCommand> = (0..24)
        .map(|c| ClientCommand {
            client: c,
            sn: 1,
            op: 1,
        })
        .collect();
    let mut surges = Surges::new(&[0.0, 0.0, 0.05, 0.15, 0.3, 0.5]);

    for round in 1..=ROUNDS {
        let mask = surges.mask(&mut rng);

        // Alg 5 step 1, with an equivocating second command injected at random.
        let mut delivered = Vec::new();
        for c in clients.iter_mut() {
            let mut offers = vec![*c];
            if rng.random_bool(0.03) {
                offers.push(ClientCommand {
                    op: c.op + EQUIVOCATE,
                    ..*c
                });
            }
            for x in offers {
                let target = rng.random_range(0..N);
                if mask[target] {
                    continue;
                }
                let triage = refs[target].on_client_command(&x);
                assert_eq!(
                    prod[target].on_client_command(&x),
                    triage,
                    "seed {seed} round {round}: triage"
                );
                match triage {
                    Triage::Amplify => delivered.push(x),
                    Triage::AckCommitted if x == *c => {
                        *c = ClientCommand {
                            sn: c.sn + 1,
                            op: rng.random_range(0..1000),
                            ..*c
                        };
                    }
                    _ => {}
                }
            }
        }
        let appends = appends_for(&delivered, &mask, round, &mut rng);

        // Alg 5 steps 2-3: all replies read the pre-step states.
        let mut prod_in: Vec<Vec<CompactReply>> = vec![Vec::new(); N];
        let mut ref_in: Vec<Vec<spec::CompactReply>> = vec![Vec::new(); N];
        for i in 0..N {
            let b_i = refs[i].b_i();
            assert_eq!(prod[i].b_i(), b_i, "seed {seed} round {round}: b_{i}");
            for _ in 0..cfg.k {
                let j = rng.random_range(0..N);
                if mask[i] || mask[j] {
                    continue;
                }
                match (prod[j].answer(b_i), refs[j].answer(b_i)) {
                    (Some(reply), Some(expected)) => {
                        assert_eq!(
                            (reply.s_j.is_some(), expected.s_j.is_some()),
                            (b_i, b_i),
                            "seed {seed} round {round}: S_{j} is attached iff b_{i} = 1"
                        );
                        prod_in[i].push(reply);
                        ref_in[i].push(expected);
                    }
                    (None, None) => {}
                    _ => panic!("seed {seed} round {round}: node {j} replies on one side only"),
                }
            }
        }

        // Alg 5 steps 4-5, M drawn once in ascending node order and fed to both.
        for i in 0..N {
            let expected = prod[i].draw_reply_choice(ref_in[i].len(), &mut rng.clone());
            let m = spec::choose_m(ref_in[i].len(), cfg.ell, &mut rng);
            assert_eq!(expected, m, "seed {seed} round {round}: M for node {i}");

            let b_i = refs[i].b_i();
            match &m {
                None if !ref_in[i].is_empty() => {
                    cov.below_ell_with_reply += 1;
                    cov.below_ell_state_adopted += u64::from(b_i);
                }
                None => {}
                Some(m) => {
                    cov.step4_state_adopted += u64::from(b_i);
                    let logs: Vec<Vec<Timed>> =
                        m.iter().map(|&j| ref_in[i][j].l_j.clone()).collect();
                    cov.drawn_logs_with_repeated_entries += logs
                        .iter()
                        .filter(|l| {
                            l.iter()
                                .enumerate()
                                .any(|(a, x)| l[..a].iter().any(|y| y.entry == x.entry))
                        })
                        .count() as u64;
                    let merged = spec::median_merge_with(&logs, &appends[i], policy);
                    cov.nulled_duplicate += u64::from(!spec::conflicting(&merged).is_empty());
                    let drawn: Vec<&ChunkSeq<Timed>> =
                        m.iter().map(|&j| &**prod_in[i][j].l_j.entries()).collect();
                    cov.shared_chunks += u64::from(shares_chunks(&drawn));
                }
            }
            let before = refs[i].s_i.executed.len();

            prod[i].step_chosen(&prod_in[i], &appends[i], round, m.as_deref());
            refs[i].step(&ref_in[i], &appends[i], round, m.as_deref());

            cov.executed_commands += refs[i].s_i.executed.len().saturating_sub(before) as u64;
            let at = format!("seed {seed} round {round} node {i}");
            assert_eq!(prod[i].log_entries(), refs[i].l_i, "{at}: log");
            assert!(
                prod[i].log_perm_arc().is_none_or(Log::is_consistent),
                "{at}: the carried index does not sort the log"
            );
            assert_eq!(state_of(prod[i].shared_state()), refs[i].s_i, "{at}: state");
        }
    }
}

#[test]
fn compact_nodes_match_the_reference_every_round() {
    let mut cov = CompactCoverage::default();
    for seed in 0..SEEDS {
        compact_world(seed, MergePolicy::default(), &mut cov);
    }
    eprintln!("compact coverage over {SEEDS} seeds x {ROUNDS} rounds x {N} nodes: {cov:?}");
    assert!(cov.below_ell_with_reply >= 100, "{cov:?}");
    assert!(cov.below_ell_state_adopted >= 50, "{cov:?}");
    assert!(cov.step4_state_adopted >= 100, "{cov:?}");
    assert!(cov.nulled_duplicate >= 100, "{cov:?}");
    assert!(cov.shared_chunks >= 1000, "{cov:?}");
    assert!(cov.executed_commands >= 10_000, "{cov:?}");
}

/// The non-default merge readings run in the experiments' loads, so each gets the same demand as the default.
#[test]
fn compact_nodes_match_the_reference_under_every_merge_policy() {
    for (stamp, union) in [
        (StampTie::IncludeRound, UnionStamp::LastSighting),
        (StampTie::CommandsOnly, UnionStamp::FirstSighting),
        (StampTie::CommandsOnly, UnionStamp::LastSighting),
    ] {
        let mut cov = CompactCoverage::default();
        for seed in 0..SEEDS / 2 {
            compact_world(seed, MergePolicy { stamp, union }, &mut cov);
        }
        eprintln!("{stamp:?}/{union:?}: {cov:?}");
        assert!(cov.nulled_duplicate >= 50, "{stamp:?}/{union:?}: {cov:?}");
        assert!(
            cov.drawn_logs_with_repeated_entries >= 50,
            "{stamp:?}/{union:?}: {cov:?}"
        );
    }
}

#[derive(Debug, Default)]
struct RecoveryCoverage {
    below_ell: u64,
    logless_at_ell: u64,
    all_reset_replies: u64,
    adopted: u64,
    adopted_at_tied_window: u64,
    tie_with_differing_checkpoints: u64,
    rollback_to_checkpoint: u64,
    rollback_to_genesis: u64,
    boundary_with_bot_log: u64,
    shared_mint: u64,
    shared_chunks: u64,
    executed_commands: u64,
    runs_ended_by_prefix_violation: u64,
}

struct RecoveryWorld {
    seed: u64,
    cfg: Config,
    t_window: u64,
    prod: Vec<RecoveryNode>,
    refs: Vec<RecoveryServer>,
    rng: ChaCha12Rng,
}

impl RecoveryWorld {
    fn new(seed: u64) -> Self {
        let cfg = config(seed);
        let t_window = 5;
        RecoveryWorld {
            seed,
            cfg,
            t_window,
            prod: (0..N)
                .map(|_| RecoveryNode::new(cfg, t_window, false))
                .collect(),
            refs: (0..N)
                .map(|_| RecoveryServer::new(cfg.ell, t_window))
                .collect(),
            rng: ChaCha12Rng::seed_from_u64(seed ^ 0x5eed),
        }
    }

    fn compare(&self, i: usize, at: &str) {
        let (prod, reference) = (&self.prod[i], &self.refs[i]);
        let at = format!("seed {} {at} node {i}", self.seed);
        assert_eq!(prod.log_entries(), reference.l_i, "{at}: log");
        assert!(
            prod.log_perm_arc().is_none_or(Log::is_consistent),
            "{at}: the carried index does not sort the log"
        );
        assert_eq!(state_of(prod.shared_state()), reference.s_i, "{at}: state");
        assert_eq!(prod.reset_state(), reference.r_i, "{at}: R");
        assert_eq!(
            checkpoint_of(prod.checkpoint()),
            reference.c_i,
            "{at}: checkpoint"
        );
    }

    fn in_window(
        &mut self,
        round: u64,
        mask: &[bool],
        appends: &[Vec<(ClientCommand, u64)>],
        cov: &mut RecoveryCoverage,
    ) {
        // Alg 6 steps 1-2: all replies read the pre-step states.
        let mut prod_in: Vec<Vec<RecoveryReply>> = vec![Vec::new(); N];
        let mut ref_in: Vec<Vec<spec::RecoveryReply>> = vec![Vec::new(); N];
        for i in 0..N {
            for _ in 0..self.cfg.k {
                let j = self.rng.random_range(0..N);
                if mask[i] || mask[j] {
                    continue;
                }
                let prod_reply = self.prod[j].answer();
                match self.refs[j].answer() {
                    Some(reply) => {
                        let prod_reply = prod_reply.unwrap_or_else(|| {
                            panic!("seed {} round {round}: node {j} does not answer", self.seed)
                        });
                        assert_eq!(
                            prod_reply.l_j.as_ref().map(|l| l.entries().to_vec()),
                            reply.l_j,
                            "seed {} round {round}: node {j} reply log",
                            self.seed
                        );
                        prod_in[i].push(prod_reply);
                        ref_in[i].push(reply);
                    }
                    None => assert!(
                        prod_reply.is_none(),
                        "seed {} round {round}: node {j} answers while R = ⊥",
                        self.seed
                    ),
                }
            }
        }

        for i in 0..N {
            let bearing = ref_in[i].iter().filter(|r| r.l_j.is_some()).count();
            let expected = self.prod[i].draw_reply_choice(&prod_in[i], &mut self.rng.clone());
            let m = spec::choose_m(bearing, self.cfg.ell, &mut self.rng);
            assert_eq!(
                expected, m,
                "seed {} round {round}: M for node {i}",
                self.seed
            );
            self.count_step(&ref_in[i], &prod_in[i], m.as_deref(), i, cov);

            self.prod[i].step_chosen(&prod_in[i], &appends[i], m.as_deref());
            self.refs[i].step(&ref_in[i], &appends[i], m.as_deref());
            self.compare(i, &format!("round {round}"));
        }
    }

    fn count_step(
        &self,
        replies: &[spec::RecoveryReply],
        prod_in: &[RecoveryReply],
        m: Option<&[usize]>,
        i: usize,
        cov: &mut RecoveryCoverage,
    ) {
        if replies.len() < self.cfg.ell {
            cov.below_ell += 1;
            return;
        }
        cov.logless_at_ell += u64::from(m.is_none());
        cov.all_reset_replies += u64::from(replies.iter().all(|r| r.r_j == RState::Reset));
        let w_max = replies.iter().map(|r| r.c_j.w).max().unwrap();
        let newest: Vec<&spec::Checkpoint> = replies
            .iter()
            .map(|r| &r.c_j)
            .filter(|c| c.w == w_max)
            .collect();
        if w_max > self.refs[i].c_i.w {
            cov.adopted += 1;
            cov.adopted_at_tied_window += u64::from(newest.len() > 1);
            cov.tie_with_differing_checkpoints += u64::from(newest.first() != newest.last());
        }
        if let Some(m) = m {
            let logs: Vec<&ChunkSeq<Timed>> = prod_in
                .iter()
                .filter_map(|r| r.l_j.as_ref())
                .map(|l| &**l.entries())
                .collect();
            let drawn: Vec<&ChunkSeq<Timed>> = m.iter().map(|&j| logs[j]).collect();
            cov.shared_chunks += u64::from(shares_chunks(&drawn));
        }
    }

    /// The simulator's preflight: a P that is not a prefix of L_i ends the run (a w.h.p. failure).
    fn boundary_preflight_ok(&self) -> bool {
        (0..N).all(|i| {
            let reference = &self.refs[i];
            let log = match reference.r_i {
                RState::Reset => &reference.c_i.p,
                _ => &reference.l_i,
            };
            let p = reference.c_i.p.as_deref().unwrap_or(&[]);
            let ok = log.as_ref().is_none_or(|l| spec::is_command_prefix(p, l));
            assert_eq!(
                self.prod[i].validate_boundary().is_ok(),
                ok,
                "seed {} preflight",
                self.seed
            );
            ok
        })
    }

    /// Between-window steps, minting shared checkpoints for proven-equal nodes as the simulator does.
    fn boundary(&mut self, round: u64, cov: &mut RecoveryCoverage) {
        let next_window = round / self.t_window;
        let mut minted: Vec<(Arc<Checkpoint>, Vec<Timed>, Arc<Checkpoint>)> = Vec::new();
        for i in 0..N {
            if self.refs[i].r_i == RState::Reset {
                match self.refs[i].c_i.p {
                    Some(_) => cov.rollback_to_checkpoint += 1,
                    None => cov.rollback_to_genesis += 1,
                }
            }
            let before = self.refs[i].s_i.executed.len();
            let held = self.prod[i].checkpoint_shared();
            let pre = self.prod[i].would_carry_pre(round);
            let class = pre.as_ref().and_then(|pre| {
                minted
                    .iter()
                    .find(|(old, p, _)| Arc::ptr_eq(old, &held) && p == pre)
            });
            match class {
                Some((_, _, cp)) => {
                    cov.shared_mint += 1;
                    self.prod[i].end_window_shared(next_window, round, &Arc::clone(cp));
                }
                None => {
                    self.prod[i].end_window(next_window, round);
                    if let Some(pre) = pre {
                        minted.push((held, pre, self.prod[i].checkpoint_shared()));
                    }
                }
            }
            self.refs[i].end_window(next_window, round);
            cov.boundary_with_bot_log += u64::from(self.refs[i].l_i.is_none());
            cov.executed_commands += self.refs[i].s_i.executed.len().saturating_sub(before) as u64;
            self.compare(i, &format!("boundary {round}"));
        }
    }
}

fn recovery_world(seed: u64, cov: &mut RecoveryCoverage) {
    let mut world = RecoveryWorld::new(seed);
    let mut surges = Surges::new(&[0.0, 0.0, 0.0, 0.1, 0.2, 0.5, 1.0]);
    let mut sent: Vec<ClientCommand> = Vec::new();

    for round in 1..=ROUNDS {
        let mask = surges.mask(&mut world.rng);

        // Fresh unique commands (§6 assumes uniqueness), plus resends of old ones.
        let mut delivered = Vec::new();
        for _ in 0..4 {
            let rng = &mut world.rng;
            let x = if !sent.is_empty() && rng.random_bool(0.3) {
                sent[rng.random_range(0..sent.len())]
            } else {
                let id = sent.len() as u32;
                sent.push(ClientCommand {
                    client: id,
                    sn: 1,
                    op: u64::from(id),
                });
                sent[id as usize]
            };
            let target = rng.random_range(0..N);
            if mask[target] {
                continue;
            }
            let wants = world.refs[target].wants_amplify(&x);
            assert_eq!(
                world.prod[target].wants_amplify(&x),
                wants,
                "seed {seed} round {round}"
            );
            if wants {
                delivered.push(x);
            }
        }
        let appends = appends_for(&delivered, &mask, round, &mut world.rng);

        world.in_window(round, &mask, &appends, cov);
        if round.is_multiple_of(world.t_window) {
            if !world.boundary_preflight_ok() {
                cov.runs_ended_by_prefix_violation += 1;
                return;
            }
            world.boundary(round, cov);
        }
    }
}

#[test]
fn recovery_nodes_match_the_reference_every_round_and_boundary() {
    let mut cov = RecoveryCoverage::default();
    for seed in 0..SEEDS {
        recovery_world(seed, &mut cov);
    }
    eprintln!("recovery coverage over {SEEDS} seeds x {ROUNDS} rounds x {N} nodes: {cov:?}");
    assert!(cov.below_ell >= 100, "{cov:?}");
    assert!(cov.logless_at_ell >= 100, "{cov:?}");
    assert!(cov.all_reset_replies >= 100, "{cov:?}");
    assert!(cov.adopted >= 100, "{cov:?}");
    assert!(cov.adopted_at_tied_window >= 100, "{cov:?}");
    assert!(cov.tie_with_differing_checkpoints >= 20, "{cov:?}");
    assert!(cov.rollback_to_checkpoint >= 100, "{cov:?}");
    assert!(cov.rollback_to_genesis >= 20, "{cov:?}");
    assert!(cov.boundary_with_bot_log >= 100, "{cov:?}");
    assert!(cov.shared_mint >= 100, "{cov:?}");
    assert!(cov.shared_chunks >= 1000, "{cov:?}");
    assert!(cov.executed_commands >= 10_000, "{cov:?}");
}
