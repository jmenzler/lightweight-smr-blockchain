use protocol::compact::{ClientCommand, Entry};
use sim::smr::{
    AttemptOutcome, ClientModel, CommandStatus, Injection, Proto, SmrScenario, SmrState,
    TrafficPhase, point_mass_pmf, prefixes_consistent, run_smr,
};
use sim::{BlockSchedule, Config};

const T: u64 = 6;

fn scenario(n: usize, seed: u64, injections: Vec<Injection>) -> SmrScenario {
    SmrScenario {
        n,
        seed,
        cfg: Config::default(),
        sigma: 5.0,
        proto: Proto::Compact { t_commit_rounds: T },
        injections,
        max_rounds: 40,
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

fn inj(round: usize, client: u32, op: u64) -> Injection {
    Injection {
        round,
        client,
        op,
        target: None,
    }
}

#[test]
fn compact_quiescent_run_ages_seed_and_stays_alive() {
    let report = run_smr(&scenario(32, 7, vec![]));
    assert_eq!(report.metrics.len(), 40);
    for m in &report.metrics {
        assert_eq!(m.nonbot_logs, 32, "no log may fail at beta = 0");
        assert_eq!(m.max_log_len, 1, "seed/dummy no-ops only");
        assert_eq!(m.max_executed_len, 0, "no-ops never enter executed");
    }
    assert!(report.safety_ok);
}

#[test]
fn compact_lifecycle_commits_everywhere() {
    let report = run_smr(&scenario(32, 9, vec![inj(2, 1, 7)]));
    let cmd = &report.commands[0];
    assert_eq!(
        cmd.delivered_round,
        Some(2),
        "beta = 0: accepted on first draw"
    );
    // Alg 5: the command joins every log in round 2 and commits once its age reaches T, at
    // the end of round 2 + T; the client's next resend, in the following round, is acknowledged.
    assert_eq!(cmd.committed_round, Some(2 + T as usize));
    assert_eq!(
        cmd.committed_ack_round,
        Some(2 + T as usize + 1),
        "client learns commitment in the round after the commit"
    );
    assert_eq!(cmd.status, CommandStatus::Complete);
    let last = report.metrics.last().unwrap();
    assert_eq!(
        (last.min_executed_len, last.max_executed_len),
        (1, 1),
        "every server executed exactly the injected command"
    );
    assert!(report.safety_ok);
}

#[test]
fn client_resends_every_round_until_committed_ack() {
    // p. 25: the client resends "until commitment is acknowledged by the contacted server":
    // one attempt per round from the injection round, none acknowledged as committed before
    // the commit, and the last one is the AckCommitted attempt.
    let report = run_smr(&scenario(32, 11, vec![inj(2, 1, 7)]));
    let cmd = &report.commands[0];
    let ack = cmd.committed_ack_round.expect("client learns commitment");
    let rounds: Vec<usize> = cmd.attempts.iter().map(|a| a.round).collect();
    assert_eq!(
        rounds,
        (2..=ack).collect::<Vec<_>>(),
        "one attempt per round"
    );
    let (last, earlier) = cmd.attempts.split_last().unwrap();
    assert_eq!(last.outcome, AttemptOutcome::AckCommitted);
    assert!(
        earlier
            .iter()
            .all(|a| a.outcome != AttemptOutcome::AckCommitted),
        "no early commitment ack: {earlier:?}"
    );
    assert!(ack > cmd.delivered_round.unwrap());
}

#[test]
fn bot_requester_recovers_shared_state_from_snapshot_peers() {
    // Node 0 is blocked while the others commit the command; when released it
    // is ⊥ with an empty state and must catch up via a peer's attached
    // snapshot state (b_i = 1 reply path).
    let mut s = scenario(32, 13, vec![inj(2, 1, 7)]);
    s.schedule = BlockSchedule::Windows(vec![sim::BlockWindow {
        start_round: 2,
        rounds: T as usize + 4,
        target: sim::BlockTarget::Nodes(vec![0]),
    }]);
    let report = run_smr(&s);
    let last = report.metrics.last().unwrap();
    assert_eq!(
        (last.min_executed_len, last.max_executed_len),
        (1, 1),
        "the recovered node's state caught up"
    );
    assert!(report.safety_ok);
}

#[test]
fn executed_prefix_checker_flags_divergence() {
    let a = Entry::Cmd(ClientCommand {
        client: 1,
        sn: 1,
        op: 7,
    });
    let b = Entry::Cmd(ClientCommand {
        client: 2,
        sn: 1,
        op: 8,
    });
    let consistent: Vec<Vec<Entry>> = vec![vec![a.clone()], vec![a.clone(), b.clone()], vec![]];
    assert!(prefixes_consistent(&consistent));
    let diverged: Vec<Vec<Entry>> = vec![vec![a.clone()], vec![b.clone(), a.clone()]];
    assert!(!prefixes_consistent(&diverged));
    let same_len_diverged: Vec<Vec<Entry>> = vec![vec![a.clone()], vec![b.clone()]];
    assert!(!prefixes_consistent(&same_len_diverged));
}

/// FNV-1a over the run's observable trajectory. Hand-rolled rather than
/// `DefaultHasher`, whose output the standard library does not promise to
/// keep stable across releases — a golden constant has to outlive toolchains.
fn digest(vals: impl IntoIterator<Item = u64>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in vals {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

#[test]
fn golden_compact_landmarks_are_stable() {
    // These constants pin the RNG stream and merge order; same-seed replay
    // tests move with a shared stream shift and cannot detect it.
    let mut s = scenario(
        64,
        424_242,
        vec![inj(2, 1, 7), inj(9, 2, 11), inj(17, 3, 13)],
    );
    s.max_rounds = 60;
    s.schedule = BlockSchedule::FreshPerRound { fraction: 0.12 };
    let report = run_smr(&s);
    let m = &report.metrics;
    assert_eq!(m.len(), 60);
    assert!(
        m.iter().all(|round| round.blocked == 7),
        "floor(0.12*64) must block exactly 7 servers per round"
    );

    // Commitment timing: the first round each command's client is told it
    // committed, plus the two broadcast landmarks around it.
    let marks = |i: usize| {
        let c = &report.commands[i];
        (
            c.delivered_round,
            c.all_logs_round,
            c.prefix_fixed_round,
            c.committed_ack_round,
        )
    };
    // `prefix_fixed_round` is None throughout by construction: compact strips
    // the committed prefix out of the log, so the extended rule's
    // shared-prefix landmark never fires here. `committed_ack_round` is this
    // rule's commitment moment.
    assert_eq!(marks(0), (Some(3), Some(6), None, Some(10)));
    assert_eq!(marks(1), (Some(9), Some(11), None, Some(16)));
    assert_eq!(marks(2), (Some(17), Some(19), None, Some(24)));

    // Log length at fixed rounds — the merge's own output size, which any
    // change to the containment test or the union walk would move.
    let shape = |r: usize| {
        let x = m[r - 1];
        (x.nonbot_logs, x.distinct_logs, x.max_log_len)
    };
    assert_eq!(shape(10), (55, 5, 2));
    assert_eq!(shape(30), (53, 1, 1));

    // The broadcast curve of the first command: holders round by round, the
    // most stream-sensitive observable the report carries.
    let spread: Vec<u32> = report.commands[0]
        .spread
        .points()
        .iter()
        .take(8)
        .map(|p| p.useful_holders)
        .collect();
    assert_eq!(spread, vec![0, 0, 19, 33, 48, 49, 49, 49]);

    // Digest of the committed trajectory: every round's executed-length pair
    // and log shape folded into one constant.
    let d = digest(m.iter().flat_map(|x| {
        [
            x.min_executed_len as u64,
            x.max_executed_len as u64,
            x.max_log_len as u64,
            x.nonbot_logs as u64,
        ]
    }));
    assert_eq!(d, 0x08b4_a58f_5e58_0688);

    let last = m.last().unwrap();
    assert_eq!((last.min_executed_len, last.max_executed_len), (3, 3));
    assert!(report.safety_ok);
}

#[test]
fn compact_safety_holds_under_blocking() {
    let mut s = scenario(64, 17, vec![inj(2, 1, 7), inj(5, 2, 9), inj(9, 3, 11)]);
    s.max_rounds = 100;
    s.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
    let report = run_smr(&s);
    assert!(report.safety_ok);
    for cmd in &report.commands {
        assert_eq!(cmd.status, CommandStatus::Complete, "op {}", cmd.op);
    }
}

/// Def 1.3 Validity (p. 6): a command in a log at the end of a round was injected by then, or some
/// server held it at the start of the round. ⊥ entries and no-ops are protocol-internal.
#[test]
fn compact_logs_hold_only_injected_or_inherited_commands() {
    // Traffic stops at round 40, so the logs drain and x_d enters them too.
    let traffic = [
        TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(3),
        },
        TrafficPhase {
            from_round: 40,
            arrivals_pmf: point_mass_pmf(0),
        },
    ];
    let mut state = SmrState::new(
        32,
        Config::default(),
        Proto::Compact { t_commit_rounds: T },
        2.0,
        21,
        &[],
        &traffic,
        ClientModel::Unique,
        None,
    );
    let ops_in_logs = |state: &SmrState| -> Vec<u64> {
        state
            .compact_logs()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|t| t.entry.op())
            .collect()
    };
    let mut at_end_of_round = vec![Vec::new()];
    for _ in 0..80 {
        state.step_fraction(0.1);
        at_end_of_round.push(ops_in_logs(&state));
    }
    let injected: std::collections::BTreeMap<u64, usize> = state
        .report()
        .commands
        .iter()
        .map(|c| (c.op, c.injection_round))
        .collect();
    for round in 1..at_end_of_round.len() {
        for op in &at_end_of_round[round] {
            let by_now = injected.get(op).is_some_and(|&r| r <= round);
            assert!(
                by_now || at_end_of_round[round - 1].contains(op),
                "round {round}: op {op} is in a log but was neither injected nor in any log before"
            );
        }
    }
    assert!(
        at_end_of_round.iter().any(|ops| ops.len() > 10),
        "the run must load the logs"
    );
}
