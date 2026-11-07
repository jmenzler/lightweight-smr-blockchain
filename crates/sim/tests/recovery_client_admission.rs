use protocol::compact::Entry;
use sim::smr::{
    AttemptOutcome, ClientModel, Injection, ManualBlock, PrefixMismatch, Proto, SmrReport,
    SmrScenario, SmrState, TrafficPhase, run_smr, run_smr_observed,
};
use sim::{BlockSchedule, Config};
use std::collections::{BTreeMap, BTreeSet};

fn recovery(t_window_rounds: u64) -> Proto {
    Proto::Recovery {
        t_window_rounds,
        resend_until_acked: false,
        prefix_mismatch: PrefixMismatch::Abort,
    }
}

fn base(n: usize, seed: u64, sigma: f64, proto: Proto, max_rounds: usize) -> SmrScenario {
    SmrScenario {
        n,
        seed,
        cfg: Config::new(6, 3).expect("valid config"),
        sigma,
        proto,
        injections: Vec::new(),
        max_rounds,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: None,
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

/// Client 1 sends sn 1 (op 7) to a blocked server and sn 2 (op 8) to a live one.
fn out_of_order_client() -> SmrScenario {
    let mut s = base(32, 7, 1.0, recovery(10), 120);
    s.injections = vec![
        Injection {
            round: 1,
            client: 1,
            op: 7,
            target: Some(0),
        },
        Injection {
            round: 2,
            client: 1,
            op: 8,
            target: Some(1),
        },
    ];
    s.manual_blocks = vec![ManualBlock {
        node: 0,
        from_round: 1,
        to_round: Some(25),
    }];
    s
}

fn command(report: &SmrReport, op: u64) -> &sim::smr::CommandReport {
    report
        .commands
        .iter()
        .find(|c| c.op == op)
        .expect("injected command is reported")
}

// A5.6 / A5.8 via A6.36, p. 26 + p. 30: sn(x) = sn(c) + 2 must be ignored, never accepted and spread.
#[test]
fn recovery_server_ignores_a_command_whose_predecessor_is_uncommitted() {
    let report = run_smr(&out_of_order_client());
    let sn1 = command(&report, 7);
    let sn2 = command(&report, 8);
    let sn1_done = sn1.executed_round.unwrap_or(usize::MAX);
    let early: Vec<_> = sn2
        .attempts
        .iter()
        .filter(|a| a.round < sn1_done && a.outcome == AttemptOutcome::Delivered)
        .collect();
    assert!(
        early.is_empty(),
        "paper Alg 5 L1a/L1c (p. 26), subsumed by Alg 6 (p. 30): sn 2 of client 1 must be ignored while sn(c) = 0; \
         code delivered and spread it before sn 1 executed (sn 1 executed round {:?}): {early:?}",
        sn1.executed_round
    );
}

// A6.36 / A6.ASSUME.8, p. 30: with sn(c) in the state machine, client 1's commands commit in sn order and sn(c) ends at 2.
#[test]
fn recovery_commits_one_clients_commands_in_sequence_number_order() {
    let scenario = out_of_order_client();
    type Snapshot = (Vec<Vec<Entry>>, Vec<Option<u64>>);
    let mut last: Option<Snapshot> = None;
    run_smr_observed(&scenario, &mut |state: &SmrState| {
        let seqs = state.executed_seqs();
        let sns = (0..scenario.n)
            .map(|i| state.node_state(i).expect("recovery").sn_get(1))
            .collect();
        last = Some((seqs, sns));
    });
    let (seqs, sns) = last.expect("observed at least one round");
    let ops: BTreeSet<Vec<u64>> = seqs
        .iter()
        .map(|seq| {
            seq.iter()
                .filter_map(|e| match e {
                    Entry::Cmd(c) if c.client == 1 => Some(c.op),
                    _ => None,
                })
                .collect()
        })
        .collect();
    assert!(
        ops.iter().all(|o| o.as_slice() == [7, 8]) && sns.iter().all(|s| *s == Some(2)),
        "paper p. 30 + Alg 5 L1a (p. 26): client 1's sn 1 (op 7) commits before sn 2 (op 8) and sn(c) ends at 2; \
         code executed client 1's ops in orders {ops:?} with final sn(c) values {:?}",
        sns.iter().collect::<BTreeSet<_>>()
    );
}

/// n = 150, beta 0.1 fresh with a 0.8 surge in rounds 71-90, half a command per round on average.
fn surge_scenario() -> SmrScenario {
    let max_rounds = 140;
    let mut s = base(150, 58, 1.0, recovery(20), max_rounds);
    s.schedule = BlockSchedule::PerRoundFractions(
        (1..=max_rounds)
            .map(|r| if (71..=90).contains(&r) { 0.8 } else { 0.1 })
            .collect(),
    );
    s.traffic = Some(vec![TrafficPhase {
        from_round: 1,
        arrivals_pmf: vec![0.5, 0.5],
    }]);
    s
}

// A5.7 / A5.8 via A6.36, p. 26 + p. 30: a server whose state already holds sn(x) for c acks or ignores x, never spreads it.
#[test]
fn recovery_server_never_accepts_a_command_its_state_already_committed() {
    let scenario = surge_scenario();
    // sn_by_round[r][i] = server i's committed sn per client after round r; stage (b) of r+1 reads it.
    let mut sn_by_round: Vec<Vec<BTreeMap<u32, u64>>> = vec![vec![BTreeMap::new(); scenario.n]];
    let report = run_smr_observed(&scenario, &mut |state: &SmrState| {
        let row = (0..scenario.n)
            .map(|i| state.node_state(i).expect("recovery").sn_iter().collect())
            .collect();
        sn_by_round.push(row);
    });
    let mut violations = Vec::new();
    for c in &report.commands {
        let sn = 1;
        for a in &c.attempts {
            if a.outcome != AttemptOutcome::Delivered {
                continue;
            }
            let committed = sn_by_round
                .get(a.round - 1)
                .and_then(|row| row[a.target as usize].get(&c.client).copied())
                .unwrap_or(0);
            if committed >= sn {
                violations.push((c.client, a.round, a.target));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "paper Alg 5 L1b/L1c (p. 26), subsumed by Alg 6 (p. 30): a server with sn(c) >= sn(x) must not accept x; \
         code delivered {} resend(s) to servers that had already committed them, first (client, round, target): {:?} \
         (terminal {:?}, exec_dup_round {:?})",
        violations.len(),
        violations.first(),
        report.terminal,
        report.exec_dup_round
    );
}
