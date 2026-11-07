//! X13: §5 commitment certificates over the recovery protocol (Algorithm 6) —
//! the RQ11 construction the paper claims in one sentence (§6 p. 30) and never
//! builds. Certificate state rides the checkpoint, issuance happens at the
//! boundary commit (Lemma 6.9), and delivery is ack-tied resend (§5 p. 25).

use protocol::certificates::{Certificate, MmrForest, leaf_hash};
use protocol::compact::{ClientCommand, Entry};
use sim::Config;
use sim::certs::CertHarness;
use sim::smr::{AttemptOutcome, ClientModel, Proto, SmrState};

const N: usize = 32;
const T: u64 = 10;
/// The recovery T-floor is LOAD-dependent, not just T_B: amplification has to
/// clear it or commitment stalls and the arms read as false refutations.
const SIGMA: f64 = 5.0;

fn rec_state(seed: u64, resend_until_acked: bool) -> SmrState {
    rec_state_with(seed, resend_until_acked, sim::smr::PrefixMismatch::Abort)
}

fn rec_state_with(
    seed: u64,
    resend_until_acked: bool,
    prefix_mismatch: sim::smr::PrefixMismatch,
) -> SmrState {
    SmrState::new_with_certs(
        N,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: T,
            resend_until_acked,
            prefix_mismatch,
        },
        SIGMA,
        seed,
        &[],
        &[],
        ClientModel::Unique,
        None,
        true,
        Default::default(),
        Default::default(),
    )
}

/// §5 p. 25 client: one command in flight, the next issued only after the
/// previous one's commit ack. Recovery has no triage serializer, so unpaced
/// injection lets one boundary batch-commit several sns of a client and evict
/// last-two window slots before their chains were ever deliverable.
struct Actor {
    client: u32,
    next_op: u64,
    issued: u64,
}

impl Actor {
    fn pump(&mut self, state: &mut SmrState, harness: &mut CertHarness) {
        if harness.acked_sn(self.client) == self.issued {
            let op = self.next_op;
            self.next_op += 1;
            state.inject(self.client, op, None).expect("inject");
            self.issued += 1;
            harness.record_issue(self.client, self.issued, op);
        }
    }
}

fn actors(clients: &[(u32, u64)], harness: &mut CertHarness) -> Vec<Actor> {
    clients
        .iter()
        .map(|&(client, next_op)| {
            harness.register_client(client);
            Actor {
                client,
                next_op,
                issued: 0,
            }
        })
        .collect()
}

/// One round: pump the clients, step under `blocked`, mirror each server's
/// checkpoint-resident cert state. Returns the servers `verify_everywhere`
/// must skip — blocked or ⊥, i.e. the complement of the useful set.
fn step_round(
    state: &mut SmrState,
    harness: &mut CertHarness,
    acts: &mut [Actor],
    blocked: &[bool],
) -> Vec<bool> {
    for a in acts.iter_mut() {
        a.pump(state, harness);
    }
    let status = state.step_masked(blocked);
    harness.observe_round_rec(state);
    (0..N)
        .map(|i| blocked[i] || status.nodes[i].log_len.is_none())
        .collect()
}

fn run_clean(state: &mut SmrState, harness: &mut CertHarness, acts: &mut [Actor], rounds: usize) {
    let no_block = vec![false; N];
    for _ in 0..rounds {
        step_round(state, harness, acts, &no_block);
    }
}

/// The X13 surge: 20 of 32 servers blocked across rounds 66–95. Each surviving
/// node then draws k = 6 targets of which ≈ 2.25 can reply — under ℓ = 3, so
/// the population spirals to all-⊥ and rolls back on release.
fn surge_mask(round: usize) -> Vec<bool> {
    (0..N)
        .map(|i| (66..=95).contains(&round) && i < 20)
        .collect()
}

/// The induction invariant on the live population: every server's checkpoint
/// forest covers exactly its own committed sequence, and every client with a
/// committed command is served from the last-two window. Read through
/// `executed_entries`, which IS the checkpoint's state (S ≡ C.S, Alg 6).
fn assert_carriage(state: &SmrState, label: &str) {
    for i in 0..N {
        let executed = state.executed_entries(i).expect("recovery executed seq");
        let certs = state.checkpoint_certs(i).expect("recovery certs");
        assert_eq!(
            certs.forest().len(),
            executed.len() as u64,
            "{label}: server {i} forest lost its committed sequence"
        );
        let mut rebuilt = MmrForest::new();
        for entry in executed.iter() {
            rebuilt.append(leaf_hash(entry));
        }
        assert_eq!(
            certs.forest().roots(),
            rebuilt.roots(),
            "{label}: server {i} forest roots differ from its own committed sequence"
        );
        let sequence: Vec<Entry> = executed.iter().cloned().collect();
        for entry in &sequence {
            let Entry::Cmd(c) = entry else {
                panic!("{label}: server {i} executed a non-command {entry:?}");
            };
            let (prev, last) = certs.last_two(c.client).unwrap_or_else(|| {
                panic!(
                    "{label}: server {i} serves no last-two window for client {}",
                    c.client
                )
            });
            for stored in prev.into_iter().chain([last]) {
                assert_stored_command_is_sound(certs, &sequence, c.client, stored, label, i);
            }
        }
    }
}

/// A stored window entry points at the command the server itself executed at that position;
/// a certificate assembled from it verifies on this server, and the same certificate with a
/// different op does not.
fn assert_stored_command_is_sound(
    certs: &protocol::certificates::ServerCertState,
    sequence: &[Entry],
    client: u32,
    stored: &protocol::certificates::StoredCommand,
    label: &str,
    server: usize,
) {
    let at = format!(
        "{label}: server {server} client {client} sn {} pos {}",
        stored.sn, stored.pos
    );
    let entry = sequence
        .get(stored.pos as usize)
        .unwrap_or_else(|| panic!("{at}: position beyond the committed sequence"));
    let Entry::Cmd(c) = entry else {
        panic!("{at}: position holds {entry:?}")
    };
    assert_eq!((c.client, c.sn), (client, stored.sn), "{at}: wrong command");
    assert_eq!(stored.leaf, leaf_hash(entry), "{at}: wrong leaf");
    let genuine = Certificate::Chained {
        entry: entry.clone(),
        pos: stored.pos,
        chain: stored.chain.clone(),
    };
    assert!(certs.verify(client, &genuine), "{at}: genuine rejected");
    let forged = Certificate::Chained {
        entry: Entry::Cmd(ClientCommand { op: c.op + 1, ..*c }),
        pos: stored.pos,
        chain: stored.chain.clone(),
    };
    assert!(!certs.verify(client, &forged), "{at}: forged op accepted");
}

/// Thm 6 over every certificate the clients hold: covered somewhere, accepted
/// by every useful server that covers it.
fn assert_every_issued_verifies(
    harness: &CertHarness,
    clients: &[u32],
    skip: &[bool],
    label: &str,
) {
    for &client in clients {
        let acked = harness.acked_sn(client);
        assert!(acked > 0, "{label}: client {client} holds no certificate");
        for sn in 1..=acked {
            let cert = harness
                .client(client)
                .build_certificate(sn)
                .unwrap_or_else(|| panic!("{label}: client {client} sn {sn} has no certificate"));
            let tally = harness.verify_everywhere(client, &cert, skip);
            assert!(
                tally.covered > 0,
                "{label}: client {client} sn {sn} covered by no useful server"
            );
            assert_eq!(
                tally.accepted, tally.covered,
                "{label}: client {client} sn {sn} accepted {}/{}",
                tally.accepted, tally.covered
            );
        }
    }
}

// --- issuance + delivery: the ack-tied resend client (P3) ---

#[test]
fn resend_until_acked_delivers_the_commit_ack_and_its_notification() {
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100)], &mut harness);
    let mut state = rec_state(4242, true);
    run_clean(&mut state, &mut harness, &mut acts, 200);

    let acked = harness.acked_sn(10);
    assert!(acked >= 3, "client must get repeated acks, got {acked}");
    assert!(
        harness.client(10).chain_for(1).is_some(),
        "sn 1's chain rides sn 2's ack (§5 p. 28)"
    );
    assert_eq!(
        harness.client(10).next_sn(),
        acked + 1,
        "the client's next sn follows its acks"
    );
}

#[test]
fn ack_mode_off_never_acks_and_leaves_the_client_stage_alone() {
    // Anti-vacuity for the gate: the default run has no ack moment at all —
    // its client loop stops at the omniscient executed_round latch.
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100)], &mut harness);
    let mut state = rec_state(4242, false);
    let no_block = vec![false; N];
    for _ in 0..200 {
        for a in acts.iter_mut() {
            if a.issued == 0 {
                state.inject(a.client, a.next_op, None).expect("inject");
                a.issued = 1;
                harness.record_issue(a.client, 1, a.next_op);
            }
        }
        state.step_masked(&no_block);
        harness.observe_round_rec(&state);
    }
    let report = state.report();
    assert!(
        report.commands.iter().any(|c| c.executed_round.is_some()),
        "the command must still commit with the mode off"
    );
    assert!(
        report
            .commands
            .iter()
            .flat_map(|c| &c.attempts)
            .all(|a| a.outcome != AttemptOutcome::AckCommitted),
        "mode off must produce no ack moments"
    );
    assert_eq!(harness.acked_sn(10), 0, "and therefore no client acks");
}

#[test]
fn post_commit_resends_never_respread_the_stripped_command() {
    // The trap: the boundary strips a committed command from every log, so an
    // unguarded post-commit resend re-amplifies it, it re-ages, and it
    // executes twice. The ack clause has to suppress that contact.
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100)], &mut harness);
    let mut state = rec_state(7, true);
    run_clean(&mut state, &mut harness, &mut acts, 200);
    assert!(
        harness.acked_sn(10) >= 3,
        "need ≥3 sequential sns to exercise the trap"
    );

    for seq in state.executed_seqs() {
        let mut seen = std::collections::BTreeSet::new();
        for e in &seq {
            assert!(seen.insert(e.clone()), "executed twice: {e:?}");
        }
    }
    for c in &state.report().commands {
        let Some(executed) = c.executed_round else {
            continue;
        };
        for a in &c.attempts {
            assert!(
                a.round <= executed || a.outcome != AttemptOutcome::Delivered,
                "op {}: round-{} resend took a delivery {} rounds after commitment \
                 — it would re-amplify the stripped command",
                c.op,
                a.round,
                a.round - executed
            );
        }
    }
}

#[test]
fn golden_recovery_cert_landmarks_are_stable() {
    // Ack mode is new stage-(b) RNG behaviour, so it gets its OWN golden
    // rather than perturbing `golden_recovery_landmarks_are_stable`. Frozen
    // values pin the whole cert pipeline: the boundary issuance cadence, the
    // one-in-flight client discipline, and the forest the checkpoints carry.
    // Regenerate only as a deliberate, commit-noted act.
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100), (11, 200)], &mut harness);
    let mut state = rec_state(90210, true);
    run_clean(&mut state, &mut harness, &mut acts, 200);
    let report = state.report();

    assert_eq!((harness.acked_sn(10), harness.acked_sn(11)), (6, 6));
    assert_eq!(report.commands.len(), 14, "12 acked + 2 in flight");
    // Injected at r, aged into P at the boundary ≥ r + T, executed at the
    // next one, acked the round after — 30 rounds per one-in-flight command.
    let cadence: Vec<(u64, usize, Option<usize>, Option<usize>)> = report
        .commands
        .iter()
        .take(6)
        .map(|c| {
            (
                c.op,
                c.injection_round,
                c.executed_round,
                c.committed_ack_round,
            )
        })
        .collect();
    assert_eq!(
        cadence,
        vec![
            (100, 1, Some(30), Some(31)),
            (200, 1, Some(30), Some(31)),
            (101, 32, Some(60), Some(61)),
            (201, 32, Some(60), Some(61)),
            (102, 62, Some(90), Some(91)),
            (202, 62, Some(90), Some(91)),
        ]
    );

    let forests: Vec<u64> = (0..N)
        .map(|i| {
            state
                .checkpoint_certs(i)
                .expect("recovery certs")
                .forest()
                .len()
        })
        .collect();
    assert_eq!(forests, vec![12; N], "every server carries the same forest");
    assert_eq!(state.committed_entries().map(|c| c.len()), Some(12));
    let forest0 = state.checkpoint_certs(0).expect("certs").forest();
    assert_eq!(forest0.peak_heights(), vec![3, 2], "m = 12 is 8 + 4");
    // Regression pin of the roots the current leaf encoding yields; the encoding itself is
    // pinned byte for byte in the protocol crate's certificate tests.
    assert_eq!(
        hex(&forest0.roots()[0]),
        "e7ffc4c87040c9ecebf57bc8bf847f6a6d15c994b47d67533a8edf60a68d2de7"
    );
    assert!(report.recovery.expect("recovery block").fork_ok);
    assert!(report.safety_ok);
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// --- X13 acceptance arms 1–4 (P4) ---

#[test]
fn arm1_fault_free_certificates_verify_on_every_useful_server() {
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100), (11, 200)], &mut harness);
    let mut state = rec_state(90210, true);
    let no_block = vec![false; N];
    let mut skip = vec![false; N];
    for _ in 0..200 {
        skip = step_round(&mut state, &mut harness, &mut acts, &no_block);
        assert!(
            harness.roots_consistent(),
            "round {}: servers agreeing on m disagree on roots",
            state.round()
        );
        assert_carriage(&state, "arm 1");
    }
    assert!(skip.iter().all(|&s| !s), "fault-free run has no ⊥ server");
    assert_every_issued_verifies(&harness, &[10, 11], &skip, "arm 1");
    assert!(
        state.checkpoint_certs(0).expect("certs").forest().len() >= 8,
        "anti-vacuity: the run must actually commit a batch of commands"
    );
}

#[test]
fn arm1_compact_parity_of_the_same_scenario_shape_is_all_green() {
    // Parity is QUALITATIVE — all-green under both rules on the same shape.
    // The two rules consume the RNG stream differently, so a byte-level
    // forest comparison across them would not mean anything.
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100), (11, 200)], &mut harness);
    let mut state = SmrState::new(
        N,
        Config::default(),
        Proto::Compact {
            t_commit_rounds: 25,
        },
        SIGMA,
        90210,
        &[],
        &[],
        ClientModel::Unique,
        None,
    );
    let no_block = vec![false; N];
    for _ in 0..200 {
        for a in acts.iter_mut() {
            a.pump(&mut state, &mut harness);
        }
        state.step_masked(&no_block);
        harness.observe_round(&state);
        assert!(harness.roots_consistent(), "compact root divergence");
    }
    assert_every_issued_verifies(&harness, &[10, 11], &no_block, "arm 1 compact");
}

#[test]
fn arm2_pre_surge_certificates_survive_the_rollback_with_no_false_attestation() {
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100), (11, 200)], &mut harness);
    let mut state = rec_state(5150, true);

    // Phase A — benign: run until both clients hold certificates, then keep
    // those exact certificate objects across the whole surge.
    let mut skip = vec![false; N];
    for round in 1..=65usize {
        skip = step_round(&mut state, &mut harness, &mut acts, &surge_mask(round));
    }
    let pre_surge: Vec<(u32, u64, Certificate)> = [10, 11]
        .iter()
        .flat_map(|&client| (1..=harness.acked_sn(client)).map(move |sn| (client, sn)))
        .map(|(client, sn)| {
            let cert = harness
                .client(client)
                .build_certificate(sn)
                .expect("pre-surge certificate");
            (client, sn, cert)
        })
        .collect();
    assert!(
        pre_surge.len() >= 4,
        "anti-vacuity: need certificates on both clients before the surge, got {}",
        pre_surge.len()
    );
    assert_every_issued_verifies(&harness, &[10, 11], &skip, "arm 2 pre-surge");
    let acked_before_surge = [harness.acked_sn(10), harness.acked_sn(11)];

    // Only ONE of the two certificate forms is durable by design. A bare
    // `Newest` certificate is §5's degenerate case for the client's most
    // recent command and expires as soon as two later commands of that client
    // commit (p. 29). Splitting the capture keeps this arm about surviving the
    // rollback instead of silently re-testing staleness.
    let (durable, ephemeral): (Vec<_>, Vec<_>) = pre_surge
        .into_iter()
        .partition(|(_, _, cert)| matches!(cert, Certificate::Chained { .. }));
    assert!(
        !durable.is_empty() && !ephemeral.is_empty(),
        "the capture must contain both certificate forms"
    );

    // Phase B/C — surge, all-⊥ spiral, release, rollback, recovery.
    let mut saw_all_bot = false;
    for round in 66..=200usize {
        skip = step_round(&mut state, &mut harness, &mut acts, &surge_mask(round));
        saw_all_bot |= skip.iter().all(|&s| s);
        assert_carriage(&state, "arm 2");
    }
    let report = state.report();
    let rec = report.recovery.as_ref().expect("recovery block");
    let rollbacks: u32 = rec.rounds.iter().map(|r| r.rollbacks).sum();
    assert!(
        rollbacks > 0,
        "anti-vacuity: the surge must force a rollback"
    );
    assert!(saw_all_bot, "anti-vacuity: the surge must ⊥ the population");
    assert!(rec.fork_ok, "checkpoint lineage forked");
    assert!(report.safety_ok);

    // THE ARM: the durable pre-surge certificates still verify on every useful
    // server — the very objects captured before the surge, checked against
    // checkpoints that were rolled back and re-committed in between.
    for (client, sn, cert) in &durable {
        let tally = harness.verify_everywhere(*client, cert, &skip);
        assert!(
            tally.covered > 0,
            "client {client} sn {sn}: pre-surge certificate covered nowhere post-recovery"
        );
        assert_eq!(
            tally.accepted, tally.covered,
            "client {client} sn {sn}: pre-surge certificate rejected post-recovery ({}/{})",
            tally.accepted, tally.covered
        );
    }

    // The ephemeral ones are rejected — and provably because their last-two
    // slot was evicted by later commits, not because a rollback lost cert
    // state: the same command re-certifies from the client's own chains.
    for (client, sn, cert) in &ephemeral {
        assert_eq!(
            harness.verify_everywhere(*client, cert, &skip).accepted,
            0,
            "client {client} sn {sn}: a bare-newest certificate must expire"
        );
        assert!(
            harness.acked_sn(*client) >= sn + 2,
            "client {client} sn {sn}: eviction requires two later commits"
        );
        let rebuilt = harness
            .client(*client)
            .build_certificate(*sn)
            .expect("the command is still the client's own");
        assert!(
            matches!(rebuilt, Certificate::Chained { .. }),
            "client {client} sn {sn}: later acks must have delivered its chain"
        );
        let tally = harness.verify_everywhere(*client, &rebuilt, &skip);
        assert!(tally.covered > 0);
        assert_eq!(
            tally.accepted, tally.covered,
            "client {client} sn {sn}: the COMMAND must stay attestable ({}/{})",
            tally.accepted, tally.covered
        );
    }

    // No false attestation: everything ever attested is in the committed
    // sequence, so nothing a rollback discarded was ever certifiable.
    let committed = state.committed_entries().expect("recovery committed seq");
    for &client in &[10u32, 11] {
        for sn in 1..=harness.acked_sn(client) {
            assert!(
                committed
                    .iter()
                    .any(|e| matches!(e, Entry::Cmd(c) if c.client == client && c.sn == sn)),
                "client {client} sn {sn} was attested but never committed"
            );
        }
    }
    // Anti-vacuity for the risk window: a command was in flight ACROSS the
    // rollback — pre-committed before it, executed only after — and it held
    // no certificate while it was revocable.
    let crossing = report
        .commands
        .iter()
        .find(|c| {
            c.delivered_round.is_some_and(|d| d < 66) && c.executed_round.is_some_and(|e| e > 100)
        })
        .expect("a command must straddle the surge and the rollback");
    let sn_of_crossing = 1 + report
        .commands
        .iter()
        .filter(|c| c.client == crossing.client && c.injection_round < crossing.injection_round)
        .count() as u64;
    let acked_then = acked_before_surge[usize::from(crossing.client == 11)];
    assert!(
        sn_of_crossing > acked_then,
        "the straddling command (op {}) already held a certificate while it was still revocable",
        crossing.op
    );
}

#[test]
fn arm3_useful_servers_agree_on_roots_at_every_happy_boundary() {
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100), (11, 200)], &mut harness);
    let mut state = rec_state(90210, true);
    let no_block = vec![false; N];
    let mut boundaries = 0;
    for round in 1..=200usize {
        let skip = step_round(&mut state, &mut harness, &mut acts, &no_block);
        if !(round as u64).is_multiple_of(T) {
            continue;
        }
        boundaries += 1;
        let useful: Vec<usize> = (0..N).filter(|&i| !skip[i]).collect();
        let reference = state
            .checkpoint_certs(useful[0])
            .expect("certs")
            .forest()
            .roots();
        for &i in &useful {
            assert_eq!(
                state.checkpoint_certs(i).expect("certs").forest().roots(),
                reference,
                "boundary {round}: server {i} disagrees on the root set"
            );
        }
        // The free oracle: §5's command forest and the display forest over
        // the committed sequence coincide — no filler ever commits, so the
        // positions and roots are the same object.
        let mut twin = MmrForest::new();
        for entry in &state.committed_entries().expect("committed seq") {
            twin.append(leaf_hash(entry));
        }
        assert_eq!(
            twin.roots(),
            reference,
            "boundary {round}: the committed-sequence twin forest diverged"
        );
    }
    assert_eq!(boundaries, 20, "every T-boundary was checked");
    assert!(
        !state
            .checkpoint_certs(0)
            .expect("certs")
            .forest()
            .roots()
            .is_empty(),
        "anti-vacuity: the final root set must be non-empty"
    );
}

#[test]
fn arm5_carriage_and_soundness_hold_every_round_under_skip_boundary_through_the_surge() {
    // The surge forces the all-⊥ spiral and a rollback; under SkipBoundary a server whose
    // pre-commit does not prefix its log skips boundaries and re-adopts later. Whatever it
    // skips, every server's forest, windows and certificates must stay sound every round.
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100), (11, 200)], &mut harness);
    let mut state = rec_state_with(5150, true, sim::smr::PrefixMismatch::SkipBoundary);
    let mut skip = vec![false; N];
    for round in 1..=200usize {
        skip = step_round(&mut state, &mut harness, &mut acts, &surge_mask(round));
        assert_carriage(&state, "arm 5");
    }
    let report = state.report();
    let rec = report.recovery.as_ref().expect("recovery block");
    assert!(
        rec.rounds.iter().map(|r| r.rollbacks).sum::<u32>() > 0,
        "anti-vacuity: the surge must force a rollback"
    );
    assert!(rec.fork_ok && report.safety_ok);
    assert_every_issued_verifies(&harness, &[10, 11], &skip, "arm 5");
}

#[test]
fn arm4_certificate_state_rides_the_rollback() {
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[(10, 100), (11, 200)], &mut harness);
    let mut state = rec_state(5150, true);
    let mut rollback_round = None;
    for round in 1..=200usize {
        step_round(&mut state, &mut harness, &mut acts, &surge_mask(round));
        // The row carrying `rollbacks` follows the boundary that performed
        // them, so this reads the state immediately after a real rollback.
        let rec = state.report().recovery.expect("recovery block");
        if rec.rounds[round - 1].rollbacks > 0 {
            rollback_round.get_or_insert(round);
            assert_carriage(&state, "arm 4 post-rollback");
            for i in 0..N {
                let certs = state.checkpoint_certs(i).expect("certs");
                for &client in &[10u32, 11] {
                    let committed_here = state
                        .executed_entries(i)
                        .expect("recovery executed seq")
                        .iter()
                        .any(|e| matches!(e, Entry::Cmd(c) if c.client == client));
                    assert_eq!(
                        certs.last_two(client).is_some(),
                        committed_here,
                        "round {round}: server {i} last-two window for client {client} \
                         disagrees with its own committed sequence"
                    );
                }
            }
        }
    }
    let round = rollback_round.expect("anti-vacuity: the surge must force a rollback");
    assert!(
        !state
            .checkpoint_certs(0)
            .expect("certs")
            .forest()
            .is_empty(),
        "anti-vacuity: the rollback at round {round} must carry a non-empty forest"
    );
}

// --- standing pins ---

#[test]
fn cert_reads_do_not_perturb_the_replay() {
    // The harness reads checkpoints; reads are RNG-free, so a run observed
    // every round must be byte-identical to the same run observed never.
    let run = |observe: bool| {
        let mut harness = CertHarness::new(N);
        let mut acts = actors(&[(10, 100), (11, 200)], &mut harness);
        let mut state = rec_state(5150, true);
        for round in 1..=150usize {
            for a in acts.iter_mut() {
                a.pump(&mut state, &mut harness);
            }
            state.step_masked(&surge_mask(round));
            // The client actor needs acks to pace itself, so the unobserved
            // arm still drains them — it just never reads a forest.
            harness.observe_round_rec(&state);
            if observe {
                let _ = state.committed_entries();
                for i in 0..N {
                    let _ = state.checkpoint_certs(i).expect("certs").forest().roots();
                }
                let _ = harness.roots_consistent();
            }
        }
        state.report()
    };
    assert_eq!(run(true), run(false), "cert reads changed the run");
}
