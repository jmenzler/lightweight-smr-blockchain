//! The E6 certificate harness over a live stepped run: forests fed from each
//! server's executed prefix, notifications on the resubmission-ack channel,
//! Thm 6 verification at useful servers, and the §5 p. 29 staleness physics.

use protocol::certificates::Certificate;
use sim::Config;
use sim::certs::CertHarness;
use sim::smr::{ClientModel, Proto, SmrState};

const N: usize = 32;

/// Wait-policy client actor: submits its next command only after the
/// previous one was acked (§4 base behavior, p. 25).
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

fn compact_state(seed: u64, op: u64) -> SmrState {
    let mut state = SmrState::new(
        N,
        Config::default(),
        Proto::Compact {
            t_commit_rounds: 25,
        },
        1.0,
        seed,
        &[],
        &[],
        ClientModel::Unique,
        None,
    );
    state.inject(1, op, None).expect("inject");
    state
}

/// Servers with executed entries in `state` (executed length ≥ 1).
fn executed_count(state: &SmrState) -> usize {
    state
        .executed_seqs()
        .iter()
        .filter(|s| !s.is_empty())
        .count()
}

#[test]
fn roots_consistent_flags_divergence_and_rebuild_recovers() {
    let no_block = vec![false; N];
    // Run A with server 0 held blocked: the rest executes op 100, server 0
    // stays empty — the harness then holds A-forests on exactly those servers.
    let mut hold_zero = vec![false; N];
    hold_zero[0] = true;
    let mut a = compact_state(1, 100);
    while executed_count(&a) < N - 1 {
        assert!(a.round() < 400, "never saw partial execution coverage");
        a.step_masked(&hold_zero);
    }
    assert!(a.executed_seqs()[0].is_empty(), "blocked server executed");
    let mut harness = CertHarness::new(N);
    harness.observe_round(&a);
    assert!(harness.roots_consistent(), "single-run feed is consistent");

    // Run B commits a different op; observing it fills the lagging servers
    // with B-forests, so equal-length forests now disagree on roots.
    let mut b = compact_state(2, 200);
    while executed_count(&b) < N {
        assert!(b.round() < 400, "run B never fully executed");
        b.step_masked(&no_block);
    }
    harness.observe_round(&b);
    assert!(
        !harness.roots_consistent(),
        "oracle must flag divergent forests"
    );

    // A fresh state's shorter (empty) executed sequences hit the
    // adoption-shrink rebuild: every forest is replayed from scratch.
    let fresh = compact_state(3, 300);
    harness.observe_round(&fresh);
    assert!(
        harness.roots_consistent(),
        "rebuild-by-replay restores consistency"
    );
    assert!(
        harness.servers().all(|s| s.forest().roots().is_empty()),
        "rebuild starts from empty forests"
    );
}

#[test]
fn certificates_verify_across_a_live_run_and_staleness_behaves() {
    let mut state = SmrState::new(
        N,
        Config::default(),
        Proto::Compact {
            t_commit_rounds: 25,
        },
        1.0,
        4242,
        &[],
        &[],
        ClientModel::Unique,
        None,
    );
    let mut harness = CertHarness::new(N);
    let mut actors = vec![
        Actor {
            client: 10,
            next_op: 100,
            issued: 0,
        },
        Actor {
            client: 11,
            next_op: 200,
            issued: 0,
        },
    ];
    for a in &actors {
        harness.register_client(a.client);
    }

    let no_block = vec![false; N];
    let mut stale: Option<Certificate> = None;
    let mut stale_at_sn = 0;
    for _ in 0..400 {
        for a in &mut actors {
            a.pump(&mut state, &mut harness);
        }
        state.step_masked(&no_block);
        harness.observe_round(&state);
        // Every round: servers agreeing on m agree on roots.
        assert!(harness.roots_consistent(), "root divergence");

        // Capture a bare-newest certificate the moment client 10 has one.
        if stale.is_none() && harness.acked_sn(10) >= 1 {
            stale_at_sn = harness.acked_sn(10);
            stale = harness.client(10).build_certificate(stale_at_sn);
        }
    }

    // Both clients made progress under sustained load.
    assert!(
        harness.acked_sn(10) >= stale_at_sn + 2,
        "need ≥2 later commits"
    );
    assert!(harness.acked_sn(11) >= 3);

    // Thm 6: every committed command verifies at every covering useful server.
    for a in &actors {
        for sn in 1..=harness.acked_sn(a.client) {
            let cert = harness
                .client(a.client)
                .build_certificate(sn)
                .unwrap_or_else(|| panic!("client {} sn {sn}", a.client));
            let tally = harness.verify_everywhere(a.client, &cert, &no_block);
            assert!(
                tally.covered > 0,
                "client {} sn {sn} covered nowhere",
                a.client
            );
            assert_eq!(
                tally.accepted, tally.covered,
                "client {} sn {sn}: {}/{} accepted",
                a.client, tally.accepted, tally.covered
            );
        }
    }

    // §5 p. 29: the captured bare-newest certificate went stale — ≥2 further
    // commits of client 10 evicted it from every last-two window.
    let stale = stale.expect("captured a newest certificate");
    assert!(matches!(stale, Certificate::Newest { .. }));
    let tally = harness.verify_everywhere(10, &stale, &no_block);
    assert!(tally.covered > 0);
    assert_eq!(
        tally.accepted, 0,
        "stale newest cert must be rejected everywhere"
    );
}

/// Replays every ack of `round` on a reference client fed from `pre`, the servers' cert
/// state at the start of the round; returns the acks whose chains differ from the harness's.
fn reply_time_mismatches(
    state: &SmrState,
    harness: &CertHarness,
    reference: &mut std::collections::BTreeMap<u32, protocol::certificates::ClientChains>,
    pre: &[protocol::certificates::ServerCertState],
) -> (usize, usize) {
    let round = state.round();
    let (mut acks, mut bad) = (0, 0);
    for cmd in &state.report().commands {
        if cmd.committed_ack_round != Some(round) {
            continue;
        }
        let Some(chains) = reference.get_mut(&cmd.client) else {
            continue;
        };
        let acker = cmd
            .attempts
            .iter()
            .find(|a| a.round == round && a.outcome == sim::smr::AttemptOutcome::AckCommitted)
            .map(|a| a.target as usize)
            .expect("ack round implies an AckCommitted attempt");
        let sn = harness.acked_sn(cmd.client);
        chains.on_commit_ack(sn, pre[acker].notification(cmd.client));
        acks += 1;
        let differs = (1..=sn).any(|k| {
            chains.chain_for(k).map(|(p, c)| (p, c.to_vec()))
                != harness
                    .client(cmd.client)
                    .chain_for(k)
                    .map(|(p, c)| (p, c.to_vec()))
        });
        bad += usize::from(differs);
    }
    (acks, bad)
}

// §5 p. 28: the reply carrying hc(x_1), p(x_1) precedes the round's commit (Alg 5 L1b), so its
// chain is the acker's start-of-round chain, not one lengthened by the same round's merges.
#[test]
fn the_ack_payload_is_the_ackers_reply_time_chain() {
    let clients = [10u32, 11, 12, 13];
    let (mut acks, mut bad) = (0, 0);
    for seed in 980_000..980_006u64 {
        let mut state = compact_state(seed, 1);
        let mut harness = CertHarness::new(N);
        let mut actors: Vec<Actor> = clients
            .iter()
            .map(|&client| Actor {
                client,
                next_op: u64::from(client) * 1000,
                issued: 0,
            })
            .collect();
        let mut reference = std::collections::BTreeMap::new();
        for a in &actors {
            harness.register_client(a.client);
            reference.insert(
                a.client,
                protocol::certificates::ClientChains::new(a.client),
            );
        }
        let no_block = vec![false; N];
        for _ in 0..150 {
            for a in &mut actors {
                a.pump(&mut state, &mut harness);
            }
            let pre = harness.servers().cloned().collect::<Vec<_>>();
            state.step_masked(&no_block);
            harness.observe_round(&state);
            let (a, b) = reply_time_mismatches(&state, &harness, &mut reference, &pre);
            acks += a;
            bad += b;
        }
    }
    assert!(acks > 50, "too few acks: {acks}");
    assert_eq!(
        bad, 0,
        "{bad} of {acks} acks carried a chain other than the reply-time one"
    );
}

// The recovery ack-mode client is acked on the start-of-round checkpoint, so its payload is that
// checkpoint's chain even in a boundary round that installs a newer one.
#[test]
fn the_recovery_ack_payload_is_the_start_of_round_checkpoint_chain() {
    let clients = [10u32, 11, 12, 13];
    let (mut acks, mut bad) = (0, 0);
    for seed in 980_000..980_040u64 {
        let mut state = SmrState::new_with_certs(
            N,
            Config::default(),
            Proto::Recovery {
                t_window_rounds: 10,
                resend_until_acked: true,
                prefix_mismatch: sim::smr::PrefixMismatch::Abort,
            },
            5.0,
            seed,
            &[],
            &[],
            ClientModel::Unique,
            None,
            true,
            Default::default(),
            Default::default(),
        );
        let mut harness = CertHarness::new(N);
        let mut actors: Vec<Actor> = clients
            .iter()
            .map(|&client| Actor {
                client,
                next_op: u64::from(client) * 1000,
                issued: 0,
            })
            .collect();
        let mut reference = std::collections::BTreeMap::new();
        for a in &actors {
            harness.register_client(a.client);
            reference.insert(
                a.client,
                protocol::certificates::ClientChains::new(a.client),
            );
        }
        let no_block = vec![false; N];
        for _ in 0..150 {
            for a in &mut actors {
                a.pump(&mut state, &mut harness);
            }
            let pre = harness.servers().cloned().collect::<Vec<_>>();
            if state.step_masked(&no_block).failed() {
                break;
            }
            harness.observe_round_rec(&state);
            let (a, b) = reply_time_mismatches(&state, &harness, &mut reference, &pre);
            acks += a;
            bad += b;
        }
    }
    assert!(acks > 20, "too few acks: {acks}");
    assert_eq!(
        bad, 0,
        "{bad} of {acks} acks carried a chain other than the reply-time one"
    );
}
