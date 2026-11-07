//! verify_everywhere's population is Theorem 6's: servers that are not blocked and hold L_i != bottom.
//! A useful server that cannot verify is a rejection, never an omission.

use protocol::certificates::Certificate;
use sim::Config;
use sim::certs::CertHarness;
use sim::smr::{ClientModel, Proto, SmrState};

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

fn compact_state(n: usize, seed: u64, t_commit: u64) -> SmrState {
    SmrState::new(
        n,
        Config::default(),
        Proto::Compact {
            t_commit_rounds: t_commit,
        },
        1.0,
        seed,
        &[],
        &[],
        ClientModel::Unique,
        None,
    )
}

fn actors(clients: &[u32], harness: &mut CertHarness) -> Vec<Actor> {
    clients
        .iter()
        .map(|&client| {
            harness.register_client(client);
            Actor {
                client,
                next_op: 100 * u64::from(client),
                issued: 0,
            }
        })
        .collect()
}

// Thm 6, p. 28: the oracle counts a lagging useful server as a rejection.
#[test]
fn a_lagging_useful_server_is_counted_as_a_rejection() {
    const N: usize = 32;
    let mut state = compact_state(N, 0, 6);
    let mut harness = CertHarness::new(N);
    let mut acts = actors(&[1], &mut harness);
    let mut lagging_cases = 0;
    for _ in 0..400 {
        acts[0].pump(&mut state, &mut harness);
        let mask = state.sample_mask(state.blocked_count(0.1));
        let status = state.step_masked(&mask);
        harness.observe_round(&state);
        let sn = harness.acked_sn(1);
        let Some(Certificate::Chained { pos, .. }) = (sn > 0)
            .then(|| harness.client(1).build_certificate(sn))
            .flatten()
        else {
            continue;
        };
        let cert = harness.client(1).build_certificate(sn).expect("cert");
        let useful: Vec<usize> = (0..N)
            .filter(|&i| !mask[i] && status.nodes[i].log_len.is_some())
            .collect();
        let lagging = useful
            .iter()
            .filter(|&&i| {
                harness
                    .servers()
                    .nth(i)
                    .expect("server index")
                    .forest()
                    .len()
                    <= pos
            })
            .count();
        let t = harness.verify_everywhere(1, &cert, &mask);
        assert_eq!(t.covered, useful.len());
        assert!(
            t.accepted <= t.covered - lagging,
            "a lagging server cannot accept"
        );
        lagging_cases += usize::from(lagging > 0);
    }
    assert!(lagging_cases > 0, "setup: no useful server ever lagged");
}

// C.DEF.26, p. 6: bottom servers are not part of the verification population.
#[test]
fn unblocked_bottom_servers_are_outside_the_population() {
    let (n, beta, t_commit) = (32usize, 0.1, 25u64);
    let clients = [10u32, 11, 12];
    let mut state = compact_state(n, 980_000, t_commit);
    let mut harness = CertHarness::new(n);
    let mut acts = actors(&clients, &mut harness);
    let mut checked = 0;
    for _ in 0..200 {
        for a in &mut acts {
            a.pump(&mut state, &mut harness);
        }
        let mask = state.sample_mask(state.blocked_count(beta));
        let status = state.step_masked(&mask);
        harness.observe_round(&state);
        let useful = (0..n)
            .filter(|&i| !mask[i] && status.nodes[i].log_len.is_some())
            .count();
        for &c in &clients {
            let sn = harness.acked_sn(c);
            let Some(cert) = (sn > 0)
                .then(|| harness.client(c).build_certificate(sn))
                .flatten()
            else {
                continue;
            };
            let covered = harness.verify_everywhere(c, &cert, &mask).covered;
            assert_eq!(covered, useful, "client {c} sn {sn}");
            checked += 1;
        }
    }
    assert!(checked > 0, "setup: nothing was checked");
}

// C.30, p. 28: the unmasked final check must not charge rejections at non-useful servers.
#[test]
fn an_unmasked_final_check_charges_nothing_at_bottom_servers() {
    let n = 8;
    let clients = [10u32, 11, 12];
    let mut cells = 0usize;
    for seed in 0..100u64 {
        let mut state = compact_state(n, seed, 4);
        let mut harness = CertHarness::new(n);
        let mut acts = actors(&clients, &mut harness);
        let mut status = None;
        for _ in 0..(8 + seed % 20) {
            for a in &mut acts {
                a.pump(&mut state, &mut harness);
            }
            let mask = state.sample_mask(state.blocked_count(0.3));
            status = Some(state.step_masked(&mask));
            harness.observe_round(&state);
        }
        let status = status.expect("ran");
        let bottom: Vec<bool> = status.nodes.iter().map(|g| g.log_len.is_none()).collect();
        for &c in &clients {
            for sn in 1..=harness.acked_sn(c) {
                let Some(cert) = harness.client(c).build_certificate(sn) else {
                    continue;
                };
                cells += 1;
                let all = harness.verify_everywhere(c, &cert, &vec![false; n]);
                let useful = harness.verify_everywhere(c, &cert, &bottom);
                assert_eq!(all, useful, "seed {seed} client {c} sn {sn}");
            }
        }
    }
    assert!(cells > 0, "setup: no certificate was checked");
}
