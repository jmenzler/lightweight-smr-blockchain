use super::*;
use protocol::compact::{SharedState, Triage};
use std::sync::Arc;

fn client(client: u32, sn: u64) -> ClientCommand {
    ClientCommand { client, sn, op: sn }
}

/// Alg 6 "If W' > W": adopting a window-1 checkpoint sets S_i and C_i to its state together.
fn adopt_committed_sn(node: &mut RecoveryNode, client: u32, sn: u64) {
    let adopted = RecoveryReply {
        l_j: None,
        c_j: Arc::new(Checkpoint {
            s: SharedState::from_entries(Vec::new(), [(client, sn)].into()).into(),
            p: Some(Vec::new()),
            w: 1,
            certs: None,
        }),
        r_j: RState::NoReset,
    };
    node.step_chosen(&[adopted.clone(), adopted.clone(), adopted], &[], None);
    assert!(
        node.log_perm_arc().is_some(),
        "adopting P = () leaves a non-⊥ log"
    );
}

fn recovery_node_with_committed_sn(committed: Option<(u32, u64)>) -> RecoveryNode {
    let mut node = RecoveryNode::new(Config::new(6, 3).expect("config"), 8, false);
    if let Some((client, sn)) = committed {
        adopt_committed_sn(&mut node, client, sn);
    }
    node
}

// Alg 5, p. 26, literally: sn(c) starts at 0, so sn(x) = 0 = sn(c) is acknowledged, as in compact.
#[test]
fn recovery_client_triage_treats_sequence_number_zero_as_already_committed() {
    let node = recovery_node_with_committed_sn(None);

    assert_eq!(node.on_client_command(&client(1, 0)), Triage::AckCommitted);
    assert_eq!(node.on_client_command(&client(1, 1)), Triage::Amplify);
}

#[test]
fn recovery_client_triage_distinguishes_future_exact_and_stale_sequences() {
    let node = recovery_node_with_committed_sn(Some((1, 2)));

    assert_eq!(node.on_client_command(&client(1, 3)), Triage::Amplify);
    assert_eq!(node.on_client_command(&client(1, 4)), Triage::Ignore);
    assert_eq!(node.on_client_command(&client(1, 2)), Triage::AckCommitted);
    assert_eq!(node.on_client_command(&client(1, 1)), Triage::Ignore);
}

#[test]
fn recovery_client_triage_rejects_duplicate_log_entries_and_maximum_sequence_advancement() {
    let cc = client(1, 1);
    let mut duplicate = recovery_node_with_committed_sn(None);
    let reply = RecoveryReply {
        l_j: duplicate.log_perm_arc().cloned(),
        c_j: duplicate.checkpoint_shared(),
        r_j: RState::NoReset,
    };
    duplicate.step_chosen(
        &[reply.clone(), reply.clone(), reply],
        &[(cc, 1)],
        Some(&[0, 1, 2]),
    );
    let max = recovery_node_with_committed_sn(Some((2, u64::MAX)));

    assert_eq!(duplicate.on_client_command(&cc), Triage::Ignore);
    assert_eq!(
        max.on_client_command(&client(2, u64::MAX)),
        Triage::AckCommitted
    );
    assert_eq!(max.on_client_command(&client(2, 1)), Triage::Ignore);
}

#[test]
fn recovery_client_triage_does_not_ack_a_command_held_only_in_checkpoint_p() {
    let cc = client(7, 1);
    let mut node = recovery_node_with_committed_sn(None);
    let reply = RecoveryReply {
        l_j: node.log_perm_arc().cloned(),
        c_j: node.checkpoint_shared(),
        r_j: RState::NoReset,
    };
    node.step_chosen(
        &[reply.clone(), reply.clone(), reply],
        &[(cc, 1)],
        Some(&[0, 1, 2]),
    );
    node.end_window(1, 16);

    assert!(
        node.checkpoint()
            .p
            .as_ref()
            .expect("boundary carries command in P")
            .iter()
            .any(|timed| timed.entry == Entry::Cmd(cc))
    );
    assert_eq!(node.checkpoint().s.sn_get(cc.client), None);
    assert_eq!(node.on_client_command(&cc), Triage::Ignore);
}

#[test]
fn recovery_client_stage_waits_for_the_next_retry_after_a_real_boundary_commit() {
    let cc = client(7, 1);
    let t_window = 8;
    let mut state = SmrState::new(
        1,
        Config::new(6, 3).expect("config"),
        Proto::Recovery {
            t_window_rounds: t_window,
            resend_until_acked: true,
            prefix_mismatch: crate::smr::PrefixMismatch::Abort,
        },
        1.0,
        1,
        &[Injection {
            round: 1,
            client: cc.client,
            op: cc.op,
            target: Some(0),
        }],
        &[],
        ClientModel::Unique,
        None,
    );
    let no_block = [false];
    let mut p_only_round = None;
    let mut p_only_retries = 0;
    let mut committed_boundary = None;

    while state.round() < 64 && committed_boundary.is_none() {
        state.step_masked(&no_block);
        let round = state.round();
        let SmrNodes::Rec(nodes) = &state.nodes else {
            unreachable!();
        };
        let checkpoint = nodes[0].checkpoint();
        if checkpoint
            .p
            .as_ref()
            .is_some_and(|pre| pre.iter().any(|timed| timed.entry == Entry::Cmd(cc)))
            && checkpoint.s.sn_get(cc.client).is_none()
        {
            p_only_round = Some(round);
            p_only_retries += 1;
            assert_eq!(
                state.trackers[0]
                    .attempts
                    .last()
                    .map(|attempt| attempt.outcome),
                Some(AttemptOutcome::Ignored),
                "client retry must not treat P as committed S in round {round}"
            );
        }
        if checkpoint.s.sn_get(cc.client) == Some(cc.sn) {
            committed_boundary = Some(round);
            assert_eq!(
                state.trackers[0].committed_ack_round, None,
                "boundary mutation cannot ACK the client in the same step"
            );
        }
    }

    let p_only_round = p_only_round.expect("fixture reached a boundary with P but not S");
    assert!(p_only_retries > 0, "fixture made a P-only client retry");
    let committed_boundary = committed_boundary.expect("fixture committed at a later boundary");
    assert!(committed_boundary > p_only_round);
    let receivers_before_ack = state.trackers[0].amp_receivers.clone();
    state.step_masked(&no_block);
    let ack_round = state.round();

    assert!(!(ack_round as u64).is_multiple_of(t_window));
    assert_eq!(
        state.trackers[0]
            .attempts
            .last()
            .map(|attempt| attempt.outcome),
        Some(AttemptOutcome::AckCommitted)
    );
    assert_eq!(state.trackers[0].committed_ack_round, Some(ack_round));
    assert_eq!(state.trackers[0].amp_receivers, receivers_before_ack);

    let attempts_at_ack = state.trackers[0].attempts.len();
    state.step_masked(&no_block);
    assert_eq!(state.trackers[0].attempts.len(), attempts_at_ack);
}

#[test]
fn recovery_ack_evidence_is_local_until_checkpoint_adoption() {
    let cc = client(7, 1);
    let mut committed = recovery_node_with_committed_sn(None);
    let reply = RecoveryReply {
        l_j: committed.log_perm_arc().cloned(),
        c_j: committed.checkpoint_shared(),
        r_j: RState::NoReset,
    };
    committed.step_chosen(
        &[reply.clone(), reply.clone(), reply],
        &[(cc, 1)],
        Some(&[0, 1, 2]),
    );
    committed.end_window(1, 16);
    committed.end_window(2, 32);
    assert_eq!(committed.on_client_command(&cc), Triage::AckCommitted);

    let mut stale = recovery_node_with_committed_sn(None);
    assert_ne!(stale.on_client_command(&cc), Triage::AckCommitted);
    let adoption = RecoveryReply {
        l_j: committed.log_perm_arc().cloned(),
        c_j: committed.checkpoint_shared(),
        r_j: RState::NoReset,
    };
    stale.step_chosen(
        &[adoption.clone(), adoption.clone(), adoption],
        &[],
        Some(&[0, 1, 2]),
    );

    assert_eq!(stale.checkpoint().s.sn_get(cc.client), Some(cc.sn));
    assert_eq!(stale.on_client_command(&cc), Triage::AckCommitted);
}

#[test]
fn recovery_overlap_leaves_an_older_sequence_unacknowledged_after_successor_commit() {
    let node = recovery_node_with_committed_sn(Some((7, 2)));

    assert_eq!(node.on_client_command(&client(7, 1)), Triage::Ignore);
    assert_eq!(node.on_client_command(&client(7, 2)), Triage::AckCommitted);
    assert_eq!(node.on_client_command(&client(7, 3)), Triage::Amplify);
}

#[test]
fn recovery_client_stage_rejects_blocked_and_bottom_targets_before_triage() {
    let cc = client(1, 1);
    let mut state = SmrState::new(
        1,
        Config::new(6, 3).expect("config"),
        Proto::Recovery {
            t_window_rounds: 8,
            resend_until_acked: true,
            prefix_mismatch: crate::smr::PrefixMismatch::Abort,
        },
        1.0,
        1,
        &[Injection {
            round: 1,
            client: cc.client,
            op: cc.op,
            target: Some(0),
        }],
        &[],
        ClientModel::Unique,
        None,
    );
    {
        let SmrNodes::Rec(nodes) = &mut state.nodes else {
            unreachable!();
        };
        adopt_committed_sn(&mut nodes[0], cc.client, cc.sn);
    }

    state.step_masked(&[true]);
    assert_eq!(
        state.trackers[0].attempts[0].outcome,
        AttemptOutcome::TargetBlocked
    );

    {
        let SmrNodes::Rec(nodes) = &mut state.nodes else {
            unreachable!();
        };
        nodes[0].step_chosen(&[], &[], None);
    }
    state.step_masked(&[false]);
    assert_eq!(
        state.trackers[0].attempts[1].outcome,
        AttemptOutcome::TargetBot
    );
}

fn entry(op: u64) -> Entry {
    Entry::Cmd(protocol::compact::ClientCommand {
        client: op as u32,
        sn: 1,
        op,
    })
}

// Empty suffices: these checkpoints forgot nothing, so the chain folds from the basis.
fn canon() -> CanonicalOrder {
    CanonicalOrder::new()
}

#[test]
fn healthy_boundary_does_not_retain_the_test_snapshot() {
    let mut state = SmrState::new_with_certs(
        16,
        Config::new(6, 3).expect("config"),
        Proto::Recovery {
            t_window_rounds: 10,
            resend_until_acked: false,
            prefix_mismatch: crate::smr::PrefixMismatch::Abort,
        },
        1.0,
        7,
        &[],
        &[],
        ClientModel::Unique,
        None,
        false,
        Default::default(),
        Default::default(),
    );
    for _ in 0..10 {
        state.step_fraction(0.0);
    }
    assert!(state.failure.is_none());
    assert!(
        state.rec_boundary_snapshot.is_none(),
        "healthy boundaries must release test-only Arc handles before mutation"
    );
}

#[test]
fn failed_boundary_preflight_mutates_no_node_at_the_boundary() {
    let n = 598;
    let mut state = SmrState::new_with_certs(
        n,
        Config::new(6, 3).expect("config"),
        Proto::Recovery {
            t_window_rounds: 20,
            resend_until_acked: true,
            prefix_mismatch: crate::smr::PrefixMismatch::Abort,
        },
        1.0,
        1,
        &[
            Injection {
                round: 2,
                client: 1,
                op: 1,
                target: None,
            },
            Injection {
                round: 2,
                client: 2,
                op: 2,
                target: None,
            },
        ],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.0, 0.0, 1.0],
        }],
        ClientModel::Unique,
        None,
        true,
        Default::default(),
        Default::default(),
    );
    let mut failed_round = None;
    for round in 1..=200 {
        state.draw_arrivals();
        if state.step_fraction(0.1).failed() {
            failed_round = Some(round);
            break;
        }
    }
    let failed_round = failed_round.expect("fixture must reach a failed boundary");
    assert_eq!(failed_round % 20, 0, "failure must occur at a boundary");
    let before = state
        .rec_boundary_snapshot
        .as_ref()
        .expect("failed pre-boundary snapshot");
    assert!(before.iter().any(|node| node.r == RState::Bot));
    let SmrNodes::Rec(nodes) = &state.nodes else {
        unreachable!()
    };
    assert_eq!(before.len(), nodes.len());
    for (id, (before, after)) in before.iter().zip(nodes).enumerate() {
        assert!(
            Arc::ptr_eq(&before.checkpoint, &after.checkpoint_shared()),
            "node {id}: checkpoint changed after failed preflight"
        );
        assert!(
            Arc::ptr_eq(&before.state, after.state_arc()),
            "node {id}: shared state changed after failed preflight"
        );
        assert_eq!(
            before.r,
            after.reset_state(),
            "node {id}: R changed after failed preflight"
        );
        match (&before.log, after.log_perm_arc()) {
            (None, None) => {}
            (Some(before), Some(after)) => {
                assert!(
                    Arc::ptr_eq(before.entries(), after.entries()),
                    "node {id}: log changed after failed preflight"
                );
                assert!(
                    Arc::ptr_eq(before.perm(), after.perm()),
                    "node {id}: log index changed after failed preflight"
                );
            }
            _ => panic!("node {id}: log bottomness changed after failed preflight"),
        }
    }
}

// p. 30: a boundary is checked over each node's whole retained suffix, not only what grew
// since the node's last check, so an order the nodes no longer hold cannot pass it.
#[test]
fn an_inconsistent_observer_order_latches_by_the_boundary() {
    let mut state = SmrState::new_with_certs(
        16,
        Config::new(6, 3).expect("config"),
        Proto::Recovery {
            t_window_rounds: 20,
            resend_until_acked: true,
            prefix_mismatch: crate::smr::PrefixMismatch::Abort,
        },
        1.0,
        3,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.0, 1.0],
        }],
        ClientModel::Unique,
        None,
        false,
        Default::default(),
        Default::default(),
    );
    for _ in 0..85 {
        state.draw_arrivals();
        assert!(!state.step_fraction(0.0).failed(), "precondition: no abort");
    }
    assert!(state.safety_ok, "precondition: a clean run");
    let SmrNodes::Rec(nodes) = &state.nodes else {
        unreachable!("a recovery state")
    };
    let lens: Vec<u64> = nodes
        .iter()
        .map(|nd| nd.shared_state().logical_len())
        .collect();
    assert!(
        lens.iter().all(|&l| l == lens[0] && l > 0),
        "precondition: equal, non-empty executed sequences: {lens:?}"
    );
    // An observer order that holds every node's length but none of its content.
    let fake: Vec<SharedState> = lens
        .iter()
        .map(|&l| {
            SharedState::from_entries((0..l).map(|p| entry(1_000_000 + p)).collect(), [].into())
        })
        .collect();
    let views: Vec<ExecutedView<'_>> = fake.iter().map(SharedState::executed).collect();
    let mut checker = crate::smr::PrefixChecker::new(16);
    assert!(checker.check_round(&views, 0));
    state.checker = checker;
    // The incremental check or, failing that, the boundary digest must latch before forgetting.
    for _ in 85..100 {
        state.draw_arrivals();
        state.step_fraction(0.0);
    }
    assert!(
        !state.safety_ok,
        "a planted inconsistent order must latch by the boundary"
    );
}

fn cp(window: u64, ops: &[u64], sn: &[(u32, u64)], pre: Option<Vec<Timed>>) -> Checkpoint {
    Checkpoint {
        s: SharedState::from_entries(
            ops.iter().copied().map(entry).collect(),
            sn.iter().copied().collect(),
        )
        .into(),
        p: pre,
        w: window,
        certs: None,
    }
}

#[test]
fn a_fork_in_any_component_at_one_window_is_caught() {
    let clean = cp(1, &[1, 2], &[(1, 1)], None);
    for forked in [
        cp(1, &[1, 3], &[(1, 1)], None),
        cp(1, &[1, 2], &[(1, 2)], None),
        cp(1, &[1, 2], &[(1, 1)], Some(Vec::new())),
    ] {
        let mut rec = RecRunState::new(10, 2, false, crate::smr::PrefixMismatch::Abort);
        rec.check_lineage(0, &clean, &mut DigestCache::new(&canon()));
        assert!(
            rec.fork_ok,
            "the first checkpoint at a window is the record"
        );
        rec.check_lineage(1, &forked, &mut DigestCache::new(&canon()));
        assert!(!rec.fork_ok, "{forked:?} agreed with {clean:?}");
    }
}

fn stamped(op: u64, round: u64) -> Timed {
    Timed {
        entry: entry(op),
        round,
    }
}

// p. 35: same-W checkpoints are identical, with P a sequence of commands (p. 31); stamps are not part of it.
#[test]
fn same_window_checkpoints_differing_only_in_p_stamps_are_no_fork() {
    let mut rec = RecRunState::new(10, 2, false, crate::smr::PrefixMismatch::Abort);
    rec.check_lineage(
        0,
        &cp(1, &[1], &[], Some(vec![stamped(7, 32)])),
        &mut DigestCache::new(&canon()),
    );
    rec.check_lineage(
        1,
        &cp(1, &[1], &[], Some(vec![stamped(7, 33)])),
        &mut DigestCache::new(&canon()),
    );
    assert!(rec.fork_ok, "x@32 and x@33 are one command sequence");
}

#[test]
fn same_window_checkpoints_whose_p_differs_in_a_command_are_a_fork() {
    let mut rec = RecRunState::new(10, 2, false, crate::smr::PrefixMismatch::Abort);
    rec.check_lineage(
        0,
        &cp(1, &[1], &[], Some(vec![stamped(7, 32)])),
        &mut DigestCache::new(&canon()),
    );
    rec.check_lineage(
        1,
        &cp(1, &[1], &[], Some(vec![stamped(8, 32)])),
        &mut DigestCache::new(&canon()),
    );
    assert!(!rec.fork_ok);
}

/// A fresh node that adopted `checkpoint` (W > 0) from three log-less replies.
fn node_holding(checkpoint: &Arc<Checkpoint>) -> RecoveryNode {
    let mut node = RecoveryNode::new(Config::new(6, 3).expect("config"), 10, false);
    let reply = RecoveryReply {
        l_j: None,
        c_j: Arc::clone(checkpoint),
        r_j: RState::NoReset,
    };
    node.step_chosen(&[reply.clone(), reply.clone(), reply], &[], None);
    assert_eq!(node.checkpoint().w, checkpoint.w, "fixture: adopted");
    node
}

// Stamps drive aging after adoption, so interning keeps them; the fork count reads commands only.
#[test]
fn newest_window_classes_count_forks_by_command_and_intern_by_stamp() {
    let at_32 = Arc::new(cp(1, &[1], &[], Some(vec![stamped(7, 32)])));
    let at_33 = Arc::new(cp(1, &[1], &[], Some(vec![stamped(7, 33)])));
    let other = Arc::new(cp(1, &[1], &[], Some(vec![stamped(8, 32)])));
    let nodes = vec![
        node_holding(&at_32),
        node_holding(&at_33),
        node_holding(&Arc::new((*at_32).clone())),
        node_holding(&other),
    ];
    let classes = newest_window_classes(&nodes, &mut DigestCache::new(&canon()));
    assert_eq!(
        classes.fork_k, 2,
        "x@32 = x@33 at command level, y is a fork"
    );
    assert_eq!(classes.max_pre_len, 1);
    assert_eq!(
        classes.representatives.len(),
        3,
        "three stamp-exact classes"
    );
    assert_eq!(classes.member_of, vec![None, None, Some(0), None]);
}

#[test]
fn a_new_window_that_does_not_extend_the_last_one_is_caught() {
    for broken in [
        cp(2, &[1, 9, 3], &[], None),
        cp(2, &[1], &[], None),
        cp(2, &[], &[], None),
    ] {
        let mut rec = RecRunState::new(10, 2, false, crate::smr::PrefixMismatch::Abort);
        rec.check_lineage(
            0,
            &cp(1, &[1, 2], &[], None),
            &mut DigestCache::new(&canon()),
        );
        rec.check_lineage(1, &broken, &mut DigestCache::new(&canon()));
        assert!(!rec.fork_ok, "{broken:?} passed as an extension");
    }
}

#[test]
fn a_clean_lineage_survives_re_presentation_and_extension() {
    let mut rec = RecRunState::new(10, 2, false, crate::smr::PrefixMismatch::Abort);
    let w1 = cp(1, &[1, 2], &[(1, 1)], None);
    for _ in 0..5 {
        rec.check_lineage(0, &w1, &mut DigestCache::new(&canon()));
        rec.check_lineage(1, &w1, &mut DigestCache::new(&canon()));
    }
    rec.check_lineage(
        0,
        &cp(2, &[1, 2, 3], &[(1, 1), (3, 1)], None),
        &mut DigestCache::new(&canon()),
    );
    rec.check_lineage(
        1,
        &cp(2, &[1, 2, 3], &[(1, 1), (3, 1)], None),
        &mut DigestCache::new(&canon()),
    );
    assert!(rec.fork_ok);
}

#[test]
fn equal_states_with_unequal_forget_lines_are_one_checkpoint() {
    let ops = [1u64, 2, 3, 4];
    let entries: Vec<Entry> = ops.iter().copied().map(entry).collect();
    let canon = CanonicalOrder::from_entries(&entries);
    let signature = |forget: u64| {
        let mut c = cp(3, &ops, &[(1, 1)], Some(Vec::new()));
        Arc::make_mut(&mut c.s).forget_committed_prefix(forget);
        ForkSignature::of(&canon, &c)
    };
    for forget in 1..=ops.len() as u64 {
        assert_eq!(
            signature(0),
            signature(forget),
            "forgetting {forget} of one committed prefix made a second checkpoint"
        );
    }
}

#[test]
fn a_divergence_above_the_forget_lines_is_still_two_checkpoints() {
    let entries: Vec<Entry> = [1u64, 2, 3, 4].iter().copied().map(entry).collect();
    let canon = CanonicalOrder::from_entries(&entries);
    let signature = |ops: &[u64], forget: u64| {
        let mut c = cp(3, ops, &[(1, 1)], None);
        Arc::make_mut(&mut c.s).forget_committed_prefix(forget);
        ForkSignature::of(&canon, &c)
    };
    for forget in 0..=2 {
        assert_ne!(
            signature(&[1, 2, 3, 4], forget),
            signature(&[1, 2, 9, 4], forget),
            "forget {forget}: a fork above the line hashed the same"
        );
    }
}

#[test]
#[should_panic(expected = "disagree on what was forgotten")]
fn comparing_the_states_themselves_would_assert_on_that_pair() {
    let ops = [1u64, 2, 3, 4];
    let a = cp(3, &ops, &[(1, 1)], None);
    let mut b = cp(3, &ops, &[(1, 1)], None);
    Arc::make_mut(&mut b.s).forget_committed_prefix(2);
    let _ = a == b;
}

fn reply(window: u64) -> RecoveryReply {
    RecoveryReply {
        l_j: None,
        c_j: Arc::new(Checkpoint {
            s: SharedState::default().into(),
            p: None,
            w: window,
            certs: None,
        }),
        r_j: RState::Reset,
    }
}

#[test]
fn a_shrinking_executed_length_is_returned() {
    let mut rec = RecRunState::new(10, 2, false, crate::smr::PrefixMismatch::Abort);
    assert_eq!(rec.check_growth(1, 5), Ok(()));
    assert_eq!(
        rec.check_growth(1, 4),
        Err(RecoveryViolation::ExecutedRegression {
            previous_len: 5,
            len: 4
        })
    );
}

#[test]
fn legal_growth_and_a_standing_length_pass() {
    let mut rec = RecRunState::new(10, 2, false, crate::smr::PrefixMismatch::Abort);
    for len in [0, 3, 3, 9] {
        assert_eq!(rec.check_growth(0, len), Ok(()));
    }
    assert_eq!(rec.prev_len, vec![9, 0]);
}

#[test]
fn the_reported_donor_is_the_reply_the_node_adopts() {
    for windows in [
        vec![0, 1, 2],
        vec![2, 1, 0],
        vec![1, 3, 3, 2],
        vec![3, 3, 3],
        vec![0, 5, 5, 5, 1],
        vec![1, 1, 1],
    ] {
        let replies: Vec<RecoveryReply> = windows.iter().map(|&w| reply(w)).collect();
        let mut node = RecoveryNode::new(Config::new(6, 3).expect("config"), 10, false);
        node.step_chosen(&replies, &[], None);
        let donor = &replies[protocol::recovery::latest_checkpoint_index(&replies)];
        assert!(
            Arc::ptr_eq(&node.checkpoint_shared(), &donor.c_j),
            "windows {windows:?}: the node adopted another reply than the reported donor"
        );
    }
}

#[test]
fn executed_growth_is_checked_past_the_u32_range() {
    let mut rec = RecRunState::new(10, 1, false, crate::smr::PrefixMismatch::Abort);
    let long = u64::from(u32::MAX) + 5;
    assert_eq!(rec.check_growth(0, long), Ok(()));
    assert_eq!(
        rec.check_growth(0, long - 2),
        Err(RecoveryViolation::ExecutedRegression {
            previous_len: long,
            len: long - 2
        })
    );
}

#[test]
#[should_panic(expected = "exceeds the u32 report field")]
fn a_report_count_past_u32_stops_the_run_instead_of_wrapping() {
    report_u32(u64::from(u32::MAX) + 1, "executed length");
}

fn recovery_state(n: usize, t_window_rounds: u64, policy: PrefixMismatch) -> SmrState {
    SmrState::new(
        n,
        Config::new(6, 3).expect("config"),
        Proto::Recovery {
            t_window_rounds,
            resend_until_acked: false,
            prefix_mismatch: policy,
        },
        1.0,
        3,
        &[],
        &[],
        ClientModel::Unique,
        None,
    )
}

fn set_node(state: &mut SmrState, i: usize, node: RecoveryNode) {
    let SmrNodes::Rec(nodes) = &mut state.nodes else {
        unreachable!();
    };
    nodes[i] = node;
}

fn checkpoint_reply(
    log: Option<Vec<Timed>>,
    checkpoint: &Arc<Checkpoint>,
    r: RState,
) -> RecoveryReply {
    RecoveryReply {
        l_j: log.map(Log::from_entries),
        c_j: Arc::clone(checkpoint),
        r_j: r,
    }
}

fn genesis() -> Arc<Checkpoint> {
    Arc::new(Checkpoint {
        s: SharedState::default().into(),
        p: None,
        w: 0,
        certs: None,
    })
}

/// A node in R = reset whose C_i is genesis: it rolls back at its next boundary.
fn resetting_node(t: u64) -> RecoveryNode {
    let mut node = RecoveryNode::new(Config::new(6, 3).expect("config"), t, false);
    let seed = vec![Timed {
        entry: Entry::Nop(0),
        round: 0,
    }];
    let votes = vec![checkpoint_reply(Some(seed), &genesis(), RState::Reset); 3];
    node.step_chosen(&votes, &[], Some(&[0, 1, 2]));
    assert_eq!(node.reset_state(), RState::Reset, "fixture: reset");
    node
}

/// R = reset with a stale C_i: a boundary skipped because P did not prefix L_i.
fn stale_reset_node(t: u64) -> RecoveryNode {
    let (a, b) = (client(1, 1), client(2, 1));
    let adopted = Arc::new(Checkpoint {
        s: SharedState::from_entries(vec![Entry::Cmd(a)], [(a.client, a.sn)].into()).into(),
        p: Some(vec![Timed {
            entry: Entry::Cmd(a),
            round: 2,
        }]),
        w: 1,
        certs: None,
    });
    let mut node = RecoveryNode::new(Config::new(6, 3).expect("config"), t, false)
        .with_prefix_mismatch(PrefixMismatch::SkipBoundary);
    node.step_chosen(
        &vec![checkpoint_reply(None, &adopted, RState::NoReset); 3],
        &[],
        None,
    );
    let other = vec![Timed {
        entry: Entry::Cmd(b),
        round: 2,
    }];
    node.step_chosen(
        &vec![checkpoint_reply(Some(other), &genesis(), RState::NoReset); 3],
        &[],
        Some(&[0, 1, 2]),
    );
    node.end_window(2, 20);
    assert!(node.stale_checkpoint(), "fixture: stale");
    assert_eq!(node.reset_state(), RState::Reset, "fixture: reset");
    node
}

fn rollbacks_after_first_boundary(mut state: SmrState, t: u64) -> (u32, u32) {
    for _ in 0..=t {
        state.step_fraction(0.0);
    }
    let row = &state.rec.as_ref().expect("recovery run").rows[t as usize];
    (row.rollbacks, row.rollback_depth)
}

#[test]
fn a_reset_node_that_rolls_back_is_counted_with_its_depth() {
    let mut state = recovery_state(1, 10, PrefixMismatch::SkipBoundary);
    set_node(&mut state, 0, resetting_node(10));
    assert_eq!(rollbacks_after_first_boundary(state, 10), (1, 1));
}

#[test]
fn a_stale_reset_node_skips_the_boundary_and_is_not_counted_as_rolling_back() {
    let mut state = recovery_state(1, 10, PrefixMismatch::SkipBoundary);
    set_node(&mut state, 0, stale_reset_node(10));
    assert_eq!(rollbacks_after_first_boundary(state, 10), (0, 0));
}

// Stage (d) samples with replacement and self allowed: a lone server hears itself k times.
#[test]
fn stage_d_samples_the_server_itself() {
    let mut state = recovery_state(1, 10, PrefixMismatch::Abort);
    state.step_fraction(0.0);
    let node = state.recovery_node(0).expect("recovery node");
    assert_eq!(node.reset_state(), RState::NoReset);
    assert!(node.log_seq().is_some(), "its own log replies reached it");
}

// A blocked server runs the between-window steps too: R = ⊥ and L = ⊥ become reset (Alg 6 last bullet).
#[test]
fn a_server_blocked_at_the_boundary_still_runs_the_between_window_steps() {
    let mut state = recovery_state(16, 10, PrefixMismatch::Abort);
    for _ in 0..9 {
        state.step_fraction(0.0);
    }
    let mut mask = vec![false; 16];
    mask[0] = true;
    state.step_masked(&mask);
    let blocked = state.recovery_node(0).expect("recovery node");
    assert_eq!(blocked.log_seq(), None);
    assert_eq!(blocked.reset_state(), RState::Reset);
    assert_eq!(blocked.checkpoint().w, 0, "no log, no mint");
    for i in 1..16 {
        assert_eq!(state.recovery_node(i).expect("node").checkpoint().w, 1);
    }
}

// Lemma 6.7, p. 35: a commit fork is two servers committing different checkpoints of one W.
#[test]
fn a_commit_fork_is_dated_at_the_second_distinct_committed_checkpoint_of_a_window() {
    let mut rec = RecRunState::new(10, 3, false, PrefixMismatch::Abort);
    let first_seen = cp(1, &[1, 2], &[(1, 1)], None);
    let committed = cp(1, &[1, 3], &[(1, 1)], None);
    let other = cp(1, &[1, 4], &[(1, 1)], None);
    rec.check_lineage(0, &first_seen, &mut DigestCache::new(&canon()));
    rec.check_lineage(1, &committed, &mut DigestCache::new(&canon()));
    assert!(
        !rec.fork_ok,
        "precondition: the lineage oracle sees two checkpoints at W = 1"
    );
    rec.record_commit(&committed, &mut DigestCache::new(&canon()), 20);
    rec.record_commit(&committed, &mut DigestCache::new(&canon()), 20);
    assert_eq!(
        rec.commit_fork_round, None,
        "the first-seen checkpoint was never committed, so one committed class is no fork"
    );
    rec.record_commit(
        &cp(2, &[1, 5], &[(1, 1)], None),
        &mut DigestCache::new(&canon()),
        30,
    );
    assert_eq!(
        rec.commit_fork_round, None,
        "another window is not compared"
    );
    rec.record_commit(&other, &mut DigestCache::new(&canon()), 40);
    rec.record_commit(&first_seen, &mut DigestCache::new(&canon()), 50);
    assert_eq!(rec.commit_fork_round, Some(40));
}

/// 16 servers, T = 10, one arrival a round, past the first commit (round 30), with the no-fork latch
/// forced off so the double-execution oracle tracks every server's own slots.
fn latched_state_with_commits() -> SmrState {
    let mut state = SmrState::new(
        16,
        Config::new(6, 3).expect("config"),
        Proto::Recovery {
            t_window_rounds: 10,
            resend_until_acked: false,
            prefix_mismatch: PrefixMismatch::Abort,
        },
        1.0,
        5,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.0, 1.0],
        }],
        ClientModel::Unique,
        None,
    );
    for _ in 0..35 {
        state.draw_arrivals();
        assert!(!state.step_fraction(0.0).failed(), "precondition: no abort");
    }
    state.safety_ok = false;
    state.draw_arrivals();
    assert!(!state.step_fraction(0.0).failed());
    state
}

// Lemma 6.9, p. 36: a shrink the double-execution oracle would also trip on ends the run as the
// recorded regression.
#[test]
fn a_shrink_after_the_latch_is_the_recorded_regression_not_an_oracle_panic() {
    let mut state = latched_state_with_commits();
    let SmrNodes::Rec(nodes) = &state.nodes else {
        unreachable!()
    };
    assert!(
        nodes[0].shared_state().logical_len() > 0,
        "precondition: server 0 executed something"
    );
    // Same W, empty S: not an adoption, so the oracle expects server 0 to have only appended.
    let emptied = Arc::new(Checkpoint {
        s: SharedState::default().into(),
        ..nodes[0].checkpoint().clone()
    });
    set_node(&mut state, 0, node_holding(&emptied));
    let mut mask = vec![false; 16];
    mask[0] = true;
    state.draw_arrivals();
    let failure = state
        .step_masked(&mask)
        .failure
        .expect("the shrink ends the run");
    assert_eq!(
        (failure.node, failure.phase),
        (0, RunFailurePhase::ExecutedRegression)
    );
}
