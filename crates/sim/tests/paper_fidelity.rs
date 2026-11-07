use sim::smr::{
    AttemptOutcome, ClientModel, Injection, Proto, SmrReport, SmrScenario, SmrState, run_smr,
    run_smr_lean,
};
use sim::spec::SmrScenarioSpec;
use sim::{BlockSchedule, Config};

const N: usize = 32;
const T_WINDOW: u64 = 8;
const SIGMA: f64 = 5.0;

fn injection(round: usize, client: u32, op: u64, target: Option<u32>) -> Injection {
    Injection {
        round,
        client,
        op,
        target,
    }
}

fn state(proto: Proto, injections: &[Injection]) -> SmrState {
    SmrState::new(
        N,
        Config::default(),
        proto,
        SIGMA,
        1,
        injections,
        &[],
        ClientModel::Unique,
        None,
    )
}

fn recovery_scenario(resend_until_acked: bool, max_rounds: usize) -> SmrScenario {
    SmrScenario {
        n: N,
        seed: 1,
        cfg: Config::default(),
        sigma: SIGMA,
        proto: Proto::Recovery {
            t_window_rounds: T_WINDOW,
            resend_until_acked,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        injections: vec![injection(1, 1, 1, None)],
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

fn parse_scenario(json: &str) -> SmrScenario {
    let spec: SmrScenarioSpec = serde_json::from_str(json).expect("scenario spec deserializes");
    SmrScenario::try_from(spec).expect("scenario validates")
}

fn future_sn_scenario(max_rounds: usize) -> SmrScenario {
    parse_scenario(&format!(
        r#"{{
            "n": 32,
            "seed": 1,
            "k": 6,
            "ell": 3,
            "sigma": 5.0,
            "proto": {{
                "kind": "recovery",
                "t_window_rounds": 8,
                "resend_until_acked": true
            }},
            "injections": [
                {{"round": 1, "client": 1, "op": 1, "target": 0}},
                {{"round": 2, "client": 1, "op": 2, "target": 1}}
            ],
            "max_rounds": {max_rounds},
            "manual_blocks": [
                {{"node": 0, "from_round": 1, "to_round": 33}}
            ]
        }}"#
    ))
}

fn command(report: &SmrReport, op: u64) -> &sim::smr::CommandReport {
    report
        .commands
        .iter()
        .find(|command| command.op == op)
        .unwrap_or_else(|| panic!("missing command op {op}"))
}

#[test]
fn control_explicit_ack_enabled_recovery_stops_after_contacted_node_ack() {
    let report = run_smr(&recovery_scenario(true, 40));
    let command = command(&report, 1);
    let ack_round = command
        .committed_ack_round
        .expect("explicit ACK mode must record a contacted-node commit acknowledgement");
    let last_attempt = command.attempts.last().expect("client made an attempt");

    assert_eq!(last_attempt.round, ack_round);
    assert_eq!(last_attempt.outcome, AttemptOutcome::AckCommitted);
    assert!(command.attempts.iter().any(|attempt| {
        attempt.round == ack_round && attempt.outcome == AttemptOutcome::AckCommitted
    }));
}

#[test]
fn control_sequential_client_issues_next_command_only_after_first_ack() {
    let proto = Proto::Recovery {
        t_window_rounds: T_WINDOW,
        resend_until_acked: true,
        prefix_mismatch: sim::smr::PrefixMismatch::Abort,
    };
    let mut state = state(proto, &[]);
    let no_block = vec![false; N];
    let first_injection_round = state.inject(7, 101, None).expect("first command injects");

    let first_ack_round = loop {
        assert!(state.round() < 40, "first command did not ack by round 40");
        state.step_masked(&no_block);
        if let Some(round) = command(&state.report(), 101).committed_ack_round {
            break round;
        }
    };
    let second_injection_round = state.inject(7, 102, None).expect("second command injects");

    let second_ack_round = loop {
        assert!(state.round() < 80, "second command did not ack by round 80");
        state.step_masked(&no_block);
        if let Some(round) = command(&state.report(), 102).committed_ack_round {
            break round;
        }
    };

    assert_eq!(first_injection_round, 1);
    assert!(second_injection_round > first_ack_round);
    assert!(second_ack_round > first_ack_round);
    let report = state.report();
    assert!(command(&report, 101).attempts.iter().any(|attempt| {
        attempt.round == first_ack_round && attempt.outcome == AttemptOutcome::AckCommitted
    }));
    assert!(command(&report, 102).attempts.iter().any(|attempt| {
        attempt.round == second_ack_round && attempt.outcome == AttemptOutcome::AckCommitted
    }));
}

#[test]
fn control_unblocked_log_holder_delivers_and_amplifies_same_round() {
    let injections = [injection(2, 1, 1, Some(0))];
    let mut state = state(Proto::Extended, &injections);
    let no_block = vec![false; N];
    let before = state.step_masked(&no_block);
    assert!(before.nodes[0].log_len.is_some());

    state.step_masked(&no_block);
    let report = state.report();
    let command = command(&report, 1);
    let attempt = command
        .attempts
        .iter()
        .find(|attempt| attempt.round == 2)
        .expect("round-2 pinned delivery attempt");

    assert_eq!(attempt.target, 0);
    assert_eq!(attempt.outcome, AttemptOutcome::Delivered);
    assert_eq!(command.delivered_round, Some(2));
    assert!(!command.amp_receivers.is_empty());
}

#[test]
fn compatibility_legacy_recovery_client_completes_without_ack_feedback() {
    let report = run_smr(&recovery_scenario(false, 40));
    let command = command(&report, 1);

    assert!(
        command.executed_round.is_some(),
        "legacy control reaches commitment"
    );
    assert_eq!(command.committed_ack_round, None);
    assert!(
        command
            .attempts
            .iter()
            .any(|attempt| attempt.outcome == AttemptOutcome::Delivered)
    );
    assert!(
        command
            .attempts
            .iter()
            .all(|attempt| attempt.outcome != AttemptOutcome::AckCommitted)
    );
}

#[test]
fn control_small_recovery_full_and_lean_runs_match_logical_state() {
    let scenario = recovery_scenario(true, 40);
    let full = run_smr(&scenario);
    let lean = run_smr_lean(&scenario);

    assert_eq!(full.terminal, lean.terminal);
    assert_eq!(full.safety_ok, lean.safety_ok);
    assert_eq!(full.failure, lean.failure);
    assert_eq!(full.metrics, lean.metrics);
    assert_eq!(full.recovery, lean.recovery);
    assert_eq!(full.commands.len(), lean.commands.len());
    for (full_command, lean_command) in full.commands.iter().zip(&lean.commands) {
        assert_eq!(full_command.client, lean_command.client);
        assert_eq!(full_command.op, lean_command.op);
        assert_eq!(full_command.status, lean_command.status);
        assert_eq!(full_command.injection_round, lean_command.injection_round);
        assert_eq!(full_command.delivered_round, lean_command.delivered_round);
        assert_eq!(full_command.all_logs_round, lean_command.all_logs_round);
        assert_eq!(
            full_command.prefix_fixed_round,
            lean_command.prefix_fixed_round
        );
        assert_eq!(
            full_command.committed_ack_round,
            lean_command.committed_ack_round
        );
        assert_eq!(full_command.executed_round, lean_command.executed_round);
    }
}

#[test]
fn extended_driver_released_bottom_target_amplifies_same_round() {
    let injections = [injection(2, 1, 1, Some(0))];
    let mut state = state(Proto::Extended, &injections);
    let mut round_one_block = vec![false; N];
    round_one_block[0] = true;
    let round_one = state.step_masked(&round_one_block);

    assert_eq!(round_one.blocked, vec![0]);
    assert_eq!(round_one.nodes[0].log_len, None);
    assert!(
        round_one.nodes[1..]
            .iter()
            .all(|node| node.log_len.is_some())
    );

    let round_two = state.step_masked(&[false; N]);
    let report = state.report();
    let command = command(&report, 1);
    let round_two_attempt = command
        .attempts
        .iter()
        .find(|attempt| attempt.round == 2)
        .expect("round-2 pinned attempt exists");

    assert_eq!(
        round_two_attempt.outcome,
        AttemptOutcome::Delivered,
        "Algorithm 3 admits a command at an unblocked server (p. 20)"
    );
    assert_eq!(command.delivered_round, Some(2));
    // Pins the shared RNG stream after bottom-path amplification adds new draws.
    assert_eq!(
        command.amp_receivers,
        vec![
            0, 2, 4, 6, 7, 11, 13, 15, 16, 17, 18, 19, 20, 21, 22, 27, 29, 30, 31
        ]
    );
    assert_eq!(round_two.nodes[0].log_len, Some(2));
    assert_eq!(report.metrics[1].distinct_logs, 2);
    // p. 19: useful = non-empty log at the START of the round and unblocked. Node 0 is bottom
    // when round 2 begins, so it is not counted although it holds a log at the round's end.
    assert_eq!(report.metrics[0].useful, (N - 1) as u32);
    assert_eq!(report.metrics[1].useful, (N - 1) as u32);
}

// p. 25: a client resends "until commitment is acknowledged by the contacted server".
#[test]
fn the_paper_recovery_client_stops_on_a_contacted_servers_commit_ack() {
    let scenario = parse_scenario(
        r#"{
            "n": 32,
            "seed": 1,
            "k": 6,
            "ell": 3,
            "sigma": 5.0,
            "proto": {"kind": "recovery", "t_window_rounds": 8, "resend_until_acked": true},
            "injections": [{"round": 1, "client": 1, "op": 1}],
            "max_rounds": 40
        }"#,
    );
    let report = run_smr(&scenario);
    let command = command(&report, 1);
    assert!(
        !command.attempts.is_empty(),
        "the paper client must make at least one delivery attempt"
    );
    let last_attempt = &command.attempts[command.attempts.len() - 1];
    let activity_terminated = last_attempt.round < 40;
    eprintln!(
        "paper client output: activity_terminated={activity_terminated}, last_attempt={last_attempt:?}, executed_round={:?}, committed_ack_round={:?}",
        command.executed_round, command.committed_ack_round
    );

    assert!(
        if activity_terminated {
            last_attempt.outcome == AttemptOutcome::AckCommitted
                && command.committed_ack_round == Some(last_attempt.round)
        } else {
            last_attempt.round == 40
        },
        "the paper client must either end with AckCommitted matching committed_ack_round or keep attempting through horizon 40; last_attempt={last_attempt:?}, executed_round={:?}, committed_ack_round={:?}",
        command.executed_round,
        command.committed_ack_round
    );
}

#[test]
fn recovery_adaptation_future_sequence_number_has_no_amplification_receivers_through_round_32() {
    let report = run_smr(&future_sn_scenario(32));
    assert_eq!(report.metrics.len(), 32);
    let first = command(&report, 1);
    assert_eq!(first.attempts.len(), 32);
    assert!(first.attempts.iter().all(|attempt| {
        attempt.target == 0 && attempt.outcome == AttemptOutcome::TargetBlocked
    }));
    let second = command(&report, 2);
    assert_eq!(second.delivered_round, None);
    assert_eq!(second.committed_ack_round, None);
    assert_eq!(
        second.attempts.len(),
        31,
        "SN 2 retries every round 2 through 32"
    );
    assert!(
        second.attempts.iter().enumerate().all(|(index, attempt)| {
            attempt.round == index + 2 && attempt.outcome == AttemptOutcome::Ignored
        }),
        "SN 2 must be retried and ignored on every round 2 through 32; attempts={:?}",
        second.attempts
    );
    let first_logical_admission = second.spread.points().iter().find(|point| {
        point.useful_holders > 0
            || point.pos_min.is_some()
            || point.pos_med.is_some()
            || point.pos_max.is_some()
    });
    eprintln!(
        "current future-SN output through round 32: delivered_round={:?}, committed_ack_round={:?}, amplification_receivers={:?}, first_logical_admission={first_logical_admission:?}, attempts={:?}",
        second.delivered_round, second.committed_ack_round, second.amp_receivers, second.attempts
    );

    assert!(
        second.amp_receivers.is_empty() && first_logical_admission.is_none(),
        "ACK-enabled recovery adaptation must neither amplify client 1 SN 2 on round-2 delivery nor admit it into any useful log/executed position through round 32 while SN 1 is blocked; receipt telemetry={:?}, amplification_receivers={:?}, first_logical_admission={first_logical_admission:?}",
        second.delivered_round,
        second.amp_receivers
    );
}

#[test]
fn recovery_adaptation_blocked_predecessor_has_no_premature_ack_through_round_48() {
    let report = run_smr(&future_sn_scenario(48));
    assert_eq!(report.metrics.len(), 48);
    let first = command(&report, 1);
    assert_eq!(first.attempts.len(), 48, "SN 1 retries through the horizon");
    assert!(
        first
            .attempts
            .iter()
            .enumerate()
            .all(|(index, attempt)| { attempt.round == index + 1 && attempt.target == 0 })
    );
    assert!(
        first
            .attempts
            .iter()
            .take(32)
            .all(|attempt| { attempt.outcome == AttemptOutcome::TargetBlocked })
    );
    assert_eq!(first.attempts[32].outcome, AttemptOutcome::TargetBot);
    assert_eq!(first.attempts[33].outcome, AttemptOutcome::Delivered);
    assert_eq!(first.attempts[34].outcome, AttemptOutcome::Delivered);
    assert!(
        first.attempts[35..]
            .iter()
            .all(|attempt| attempt.outcome == AttemptOutcome::Ignored),
        "the adapter admits the released command through its two-round snapshot transition, then later retries remain ignored; attempts={:?}",
        first.attempts
    );
    let ack_attempts: Vec<_> = first
        .attempts
        .iter()
        .filter(|attempt| attempt.outcome == AttemptOutcome::AckCommitted)
        .collect();
    eprintln!(
        "current predecessor output through round 48: delivered_round={:?}, executed_round={:?}, committed_ack_round={:?}, ack_attempts={ack_attempts:?}",
        first.delivered_round, first.executed_round, first.committed_ack_round
    );

    assert!(
        ack_attempts.is_empty() && first.committed_ack_round.is_none(),
        "op 1 cannot receive a contacted-node commit acknowledgement by horizon 48 after release 33 with T=8; current committed_ack_round={:?}, ack_attempts={ack_attempts:?}",
        first.committed_ack_round
    );
}

#[test]
fn theorem_budget_beta_point_one_blocks_at_most_floor_beta_n() {
    let scenario = SmrScenario {
        n: 598,
        seed: 1,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Extended,
        injections: Vec::new(),
        max_rounds: 1,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: None,
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    };
    let report = run_smr(&scenario);
    assert_eq!(report.metrics.len(), 1);
    assert_eq!(
        report.metrics[0].blocked, 59,
        "the theorem's at-most beta*n budget requires floor(0.1*598)=59 blocked nodes"
    );
}
