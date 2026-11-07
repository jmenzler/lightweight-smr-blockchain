use sim::smr::{ClientModel, Injection, Proto, SmrScenario, run_smr};
use sim::{BlockSchedule, Config};

fn scenario(n: usize, seed: u64, injections: Vec<Injection>) -> SmrScenario {
    SmrScenario {
        n,
        seed,
        cfg: Config::default(),
        sigma: 5.0,
        proto: Proto::Extended,
        injections,
        max_rounds: 30,
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
fn blocked_round_makes_node_bot_then_recovers() {
    // Round 2 blocks node 0 only: it sends no requests, gets no replies,
    // fails to ⊥ (Alg 3 step 5). From round 3 it samples log-holding peers
    // again and recovers via the median (step 4).
    let mut s = scenario(16, 3, vec![]);
    s.schedule = BlockSchedule::Windows(vec![sim::BlockWindow {
        start_round: 2,
        rounds: 1,
        target: sim::BlockTarget::Nodes(vec![0]),
    }]);
    let report = run_smr(&s);
    assert_eq!(report.metrics[0].nonbot_logs, 16);
    assert_eq!(report.metrics[1].nonbot_logs, 15, "blocked node fails to ⊥");
    assert_eq!(report.metrics[1].blocked, 1);
    assert_eq!(report.metrics[2].nonbot_logs, 16, "recovers next round");
    assert!(matches!(
        report.terminal,
        sim::smr::SmrTerminal::Ran { rounds: 30 }
    ));
}

#[test]
fn unblocked_bottom_target_delivers_and_amplifies() {
    let mut s = scenario(
        4,
        3,
        vec![Injection {
            round: 2,
            client: 1,
            op: 7,
            target: Some(0),
        }],
    );
    s.schedule = BlockSchedule::Windows(vec![
        sim::BlockWindow {
            start_round: 1,
            rounds: 1,
            target: sim::BlockTarget::Nodes(vec![0]),
        },
        sim::BlockWindow {
            start_round: 2,
            rounds: 1,
            target: sim::BlockTarget::Nodes(vec![1, 2, 3]),
        },
    ]);

    let report = run_smr(&s);
    let command = &report.commands[0];
    assert_eq!(command.delivered_round, Some(2));
    assert_eq!(
        command.attempts[0].outcome,
        sim::smr::AttemptOutcome::Delivered
    );
    assert!(!command.amp_receivers.is_empty());
    assert_eq!(
        report.metrics[1].nonbot_logs, 0,
        "node 0 draws no reply, so its own append request cannot keep a log (Alg 3 step 5)"
    );
}

#[test]
fn a_lone_server_draws_itself_k_times_and_keeps_its_log() {
    // Targets are k independent uniform draws over all n servers, so with n = 1 all k replies are
    // its own log. Excluding self or deduplicating targets would leave it below l replies.
    for proto in [Proto::Extended, Proto::Compact { t_commit_rounds: 6 }] {
        let mut s = scenario(1, 5, vec![inj(2, 1, 7)]);
        s.proto = proto;
        let report = run_smr(&s);
        assert!(
            report.metrics.iter().all(|m| m.nonbot_logs == 1),
            "{proto:?}: the lone server fell to ⊥"
        );
        assert_eq!(report.commands[0].delivered_round, Some(2), "{proto:?}");
        assert!(report.commands[0].all_logs_round.is_some(), "{proto:?}");
    }
}

#[test]
fn blocked_target_does_not_ack() {
    let mut s = scenario(
        4,
        3,
        vec![Injection {
            round: 2,
            client: 1,
            op: 7,
            target: Some(1),
        }],
    );
    s.schedule = BlockSchedule::Windows(vec![sim::BlockWindow {
        start_round: 2,
        rounds: 1,
        target: sim::BlockTarget::Nodes(vec![1]),
    }]);

    let report = run_smr(&s);
    let command = &report.commands[0];
    assert_ne!(command.delivered_round, Some(2));
    assert_eq!(
        command.attempts[0].outcome,
        sim::smr::AttemptOutcome::TargetBlocked
    );
}

#[test]
fn blocked_target_defers_delivery_to_a_later_round() {
    // Node 0 is blocked in round 2 (the injection round). Seeds whose round-2
    // draw hits node 0 must retry and deliver later; the rest deliver in
    // round 2. Both paths must occur across the seed range.
    let mut deferred = 0;
    let mut immediate = 0;
    for seed in 0..40 {
        let mut s = scenario(
            16,
            seed,
            vec![Injection {
                round: 2,
                client: 1,
                op: 7,
                target: None,
            }],
        );
        s.schedule = BlockSchedule::Windows(vec![sim::BlockWindow {
            start_round: 2,
            rounds: 1,
            target: sim::BlockTarget::Nodes(vec![0]),
        }]);
        let report = run_smr(&s);
        match report.commands[0].delivered_round {
            Some(2) => immediate += 1,
            Some(r) if r > 2 => deferred += 1,
            other => panic!("seed {seed}: unexpected delivery {other:?}"),
        }
    }
    assert!(immediate > 0, "some seed delivers in the injection round");
    assert!(deferred > 0, "some seed draws the blocked node and defers");
}

#[test]
fn amplify_lands_only_on_unblocked_targets_same_round() {
    // β = 0: the round the command is delivered, its append recipients are
    // exactly the distinct amp targets — and nobody else. In particular the
    // delivering server itself holds the command only if it was a random amp
    // target (no self-append: Alg 3's L̄ carries only *received* requests).
    let report = run_smr(&scenario(
        64,
        11,
        vec![Injection {
            round: 2,
            client: 1,
            op: 7,
            target: None,
        }],
    ));
    let cmd = &report.commands[0];
    assert_eq!(
        cmd.delivered_round,
        Some(2),
        "beta = 0: first draw delivers"
    );
    let recipients = &cmd.amp_receivers;
    assert!(!recipients.is_empty());
    // Entering round 3, holders are exactly the round-2 append recipients.
    let next = cmd.spread.points().iter().find(|p| p.round == 3).unwrap();
    assert_eq!(
        next.useful_holders as usize,
        recipients.len(),
        "post-delivery holders == distinct append recipients (no self-append)"
    );
}

#[test]
fn landmark_rounds_are_ordered_and_recorded() {
    let report = run_smr(&scenario(
        64,
        5,
        vec![Injection {
            round: 3,
            client: 1,
            op: 9,
            target: None,
        }],
    ));
    let cmd = &report.commands[0];
    let delivered = cmd.delivered_round.expect("delivers at beta = 0");
    let all_logs = cmd.all_logs_round.expect("broadcast completes");
    let fixed = cmd.prefix_fixed_round.expect("position fixes");
    assert!(delivered >= 3);
    assert!(all_logs >= delivered);
    assert!(fixed >= all_logs);
    assert_eq!(cmd.status, sim::smr::CommandStatus::Complete);
    assert_eq!(
        cmd.spread.points().first().unwrap().round,
        3,
        "series starts at injection"
    );
}

#[test]
fn containment_and_prefix_fixed_are_absorbing() {
    // 200 rounds at β = 0.1 after completion: the in-loop absorbing asserts
    // (panic on regression) must stay silent and the command stays Complete.
    let mut s = scenario(
        64,
        23,
        vec![Injection {
            round: 2,
            client: 1,
            op: 7,
            target: None,
        }],
    );
    s.max_rounds = 200;
    s.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
    let report = run_smr(&s);
    assert_eq!(report.commands[0].status, sim::smr::CommandStatus::Complete);
    assert!(matches!(report.terminal, sim::smr::SmrTerminal::Ran { .. }));
}

#[test]
fn undelivered_command_stays_pending_and_all_bot_terminates() {
    // Permanent full block: every log fails in round 1 → Dead terminal; the
    // round-2 injection never happens and reads Dead.
    let mut s = scenario(
        16,
        1,
        vec![Injection {
            round: 2,
            client: 1,
            op: 7,
            target: None,
        }],
    );
    s.schedule = BlockSchedule::Permanent { fraction: 1.0 };
    let report = run_smr(&s);
    assert!(matches!(
        report.terminal,
        sim::smr::SmrTerminal::Dead { round: 1 }
    ));
    assert_eq!(report.metrics.len(), 1, "no rounds simulated past death");
    let cmd = &report.commands[0];
    assert_eq!(cmd.delivered_round, None);
    assert_eq!(cmd.status, sim::smr::CommandStatus::Dead);

    // An injection past the horizon simply stays Pending.
    let s2 = scenario(
        16,
        1,
        vec![Injection {
            round: 31,
            client: 1,
            op: 7,
            target: None,
        }],
    );
    let report2 = run_smr(&s2);
    assert_eq!(report2.commands[0].status, sim::smr::CommandStatus::Pending);
    assert!(report2.commands[0].spread.points().is_empty());
}

#[test]
fn golden_run_landmarks_are_stable() {
    // Frozen values pin the stage order itself: a same-seed replay test
    // passes under any consistent reorder of the RNG contract — only these
    // golden landmarks catch one. Regenerate deliberately (and say so in the
    // commit) if the contract is ever changed on purpose.
    let mut s = scenario(
        64,
        424242,
        vec![Injection {
            round: 2,
            client: 1,
            op: 7,
            target: None,
        }],
    );
    s.max_rounds = 60;
    s.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
    let report = run_smr(&s);
    let cmd = &report.commands[0];
    assert_eq!(cmd.delivered_round, Some(2));
    assert_eq!(cmd.all_logs_round, Some(5));
    assert_eq!(cmd.prefix_fixed_round, Some(5));
    assert_eq!(cmd.amp_receivers.len(), 19);
    let p3 = cmd.spread.points().iter().find(|p| p.round == 3).unwrap();
    assert_eq!((p3.useful_holders, p3.useful_total), (18, 54));
    assert_eq!(report.metrics.last().unwrap().nonbot_logs, 58);
}

#[test]
fn same_seed_replays_identically() {
    let mut s = scenario(
        64,
        42,
        vec![Injection {
            round: 2,
            client: 1,
            op: 7,
            target: None,
        }],
    );
    s.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
    assert_eq!(run_smr(&s), run_smr(&s));
}

#[test]
fn no_injection_run_keeps_every_log_at_seed() {
    let report = run_smr(&scenario(32, 7, vec![]));
    assert_eq!(report.metrics.len(), 30, "one metrics row per round");
    for (i, m) in report.metrics.iter().enumerate() {
        assert_eq!(
            m.nonbot_logs,
            32,
            "round {}: no log may fail at beta=0",
            i + 1
        );
        assert_eq!(m.useful, 32);
        assert_eq!(m.blocked, 0);
        assert_eq!(m.distinct_logs, 1, "median of identical logs is that log");
        assert_eq!(m.max_log_len, 1, "nothing injected: logs stay [x0]");
    }
    assert!(report.commands.is_empty());
}
