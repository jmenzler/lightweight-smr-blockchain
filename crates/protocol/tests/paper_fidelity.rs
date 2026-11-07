#[path = "paper_fidelity/fixtures.rs"]
mod fixtures;
#[path = "paper_fidelity/oracle.rs"]
mod oracle;

use fixtures::{
    COMMAND_A, COMMAND_B, COMMAND_C, COMMAND_D, DonorFixture, REQUIRED_PREFIX, bare,
    compact_donors, recovery_donors,
};
use oracle::{LabeledSequence, OracleError, median_sequence};
use protocol::compact::{ClientCommand, CompactNode, CompactReply, Entry, Timed};
use protocol::merge::{StampTie, UnionStamp};
use protocol::recovery::{RState, RecoveryNode, RecoveryReply};
use protocol::{Config, LogNode};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use std::sync::Arc;

fn sequence(commands: &[ClientCommand]) -> Vec<Entry> {
    std::iter::once(Entry::Nop(0))
        .chain(commands.iter().copied().map(Entry::Cmd))
        .collect()
}

fn timed(entry: Entry, round: u64) -> Timed {
    Timed { entry, round }
}

fn compact_reply(node: &CompactNode) -> CompactReply {
    CompactReply::from_log(
        Arc::clone(node.log_arc().expect("node remains log-bearing")),
        Some(Arc::clone(node.state_arc())),
    )
}

fn recovery_reply(node: &RecoveryNode) -> RecoveryReply {
    let log = node.log_perm_arc().expect("node remains log-bearing");
    RecoveryReply {
        l_j: Some(log.clone()),
        c_j: node.checkpoint_shared(),
        r_j: node.reset_state(),
    }
}

#[test]
fn oracle_selects_hand_calculated_command_median() {
    let a = sequence(&[COMMAND_A, COMMAND_B]);
    let b = sequence(&[COMMAND_A, COMMAND_C]);
    let c = sequence(&[COMMAND_A, COMMAND_D]);

    let got = median_sequence([
        LabeledSequence::new("A", &a),
        LabeledSequence::new("B", &b),
        LabeledSequence::new("C", &c),
    ])
    .expect("the three literal sequences are in the oracle domain");

    assert_eq!(got, b);
}

#[test]
fn oracle_is_permutation_invariant() {
    let sequences = [
        sequence(&[COMMAND_A, COMMAND_B]),
        sequence(&[COMMAND_A, COMMAND_C]),
        sequence(&[COMMAND_A, COMMAND_D]),
    ];
    let labels = ["A", "B", "C"];
    let expected = sequence(&[COMMAND_A, COMMAND_C]);

    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let inputs = order.map(|index| LabeledSequence::new(labels[index], &sequences[index]));
        assert_eq!(
            median_sequence(inputs).expect("permutation keeps the same valid domain"),
            expected,
            "input order {order:?}"
        );
    }
}

#[test]
fn oracle_rejects_malformed_sequences_with_fixture_labels() {
    let duplicate = COMMAND_A;
    let cases = [
        (
            "empty",
            Vec::new(),
            OracleError::EmptySequence { fixture: "empty" },
        ),
        (
            "missing-seed",
            vec![Entry::Cmd(COMMAND_A)],
            OracleError::MissingGenesis {
                fixture: "missing-seed",
            },
        ),
        (
            "null-at-zero",
            vec![Entry::Null { client: 8, sn: 1 }],
            OracleError::NullEntry {
                fixture: "null-at-zero",
                position: 0,
            },
        ),
        (
            "other-nop-at-zero",
            vec![Entry::Nop(9)],
            OracleError::UnsupportedNop {
                fixture: "other-nop-at-zero",
                position: 0,
                value: 9,
            },
        ),
        (
            "duplicate-seed",
            vec![Entry::Nop(0), Entry::Nop(0)],
            OracleError::DuplicateGenesis {
                fixture: "duplicate-seed",
                position: 1,
            },
        ),
        (
            "null",
            vec![Entry::Nop(0), Entry::Null { client: 8, sn: 1 }],
            OracleError::NullEntry {
                fixture: "null",
                position: 1,
            },
        ),
        (
            "other-nop",
            vec![Entry::Nop(0), Entry::Nop(9)],
            OracleError::UnsupportedNop {
                fixture: "other-nop",
                position: 1,
                value: 9,
            },
        ),
        (
            "duplicate-command",
            vec![Entry::Nop(0), Entry::Cmd(duplicate), Entry::Cmd(duplicate)],
            OracleError::DuplicateCommand {
                fixture: "duplicate-command",
                first_position: 1,
                duplicate_position: 2,
                command: duplicate,
            },
        ),
    ];
    let valid_b = sequence(&[COMMAND_A, COMMAND_C]);
    let valid_c = sequence(&[COMMAND_A, COMMAND_D]);

    for (fixture, malformed, expected) in cases {
        let got = median_sequence([
            LabeledSequence::new(fixture, &malformed),
            LabeledSequence::new("B", &valid_b),
            LabeledSequence::new("C", &valid_c),
        ]);
        assert_eq!(got, Err(expected), "fixture {fixture}");
    }
}

fn expected_donor_logs() -> [Vec<Timed>; 3] {
    [
        vec![
            timed(Entry::Nop(0), 0),
            timed(Entry::Cmd(COMMAND_A), 2),
            timed(Entry::Cmd(COMMAND_B), 2),
        ],
        vec![
            timed(Entry::Nop(0), 0),
            timed(Entry::Cmd(COMMAND_A), 1),
            timed(Entry::Cmd(COMMAND_C), 2),
        ],
        vec![
            timed(Entry::Nop(0), 0),
            timed(Entry::Cmd(COMMAND_A), 3),
            timed(Entry::Cmd(COMMAND_D), 3),
        ],
    ]
}

fn assert_logs_and_uncommitted<R>(fixture: &DonorFixture<R>) {
    assert_eq!(fixture.logs, expected_donor_logs());
    assert_eq!(
        fixture.committed,
        std::array::from_fn(|_| Vec::<Entry>::new()),
        "T=100 must leave every round-1-to-3 command uncommitted"
    );
}

#[test]
fn compact_donors_reach_exact_logs_without_commitment() {
    let fixture = compact_donors();
    assert_logs_and_uncommitted(&fixture);

    let reply_logs: [Vec<Timed>; 6] =
        std::array::from_fn(|index| fixture.replies[index].l_j.entries().to_vec());
    let expected = expected_donor_logs();
    assert_eq!(
        reply_logs,
        [
            expected[0].clone(),
            expected[1].clone(),
            expected[2].clone(),
            expected[0].clone(),
            expected[1].clone(),
            expected[2].clone(),
        ]
    );
}

#[test]
fn recovery_donors_reach_exact_logs_without_commitment() {
    let fixture = recovery_donors();
    assert_logs_and_uncommitted(&fixture);

    let reply_logs: [Vec<Timed>; 6] = std::array::from_fn(|index| {
        fixture.replies[index]
            .l_j
            .as_ref()
            .expect("reachable donor remains log-bearing")
            .entries()
            .to_vec()
    });
    let expected = expected_donor_logs();
    assert_eq!(
        reply_logs,
        [
            expected[0].clone(),
            expected[1].clone(),
            expected[2].clone(),
            expected[0].clone(),
            expected[1].clone(),
            expected[2].clone(),
        ]
    );
    for reply in fixture.replies {
        assert_eq!(reply.r_j, RState::NoReset);
        assert_eq!(reply.c_j.w, 0);
        assert_eq!(reply.c_j.p, None);
        assert_eq!(reply.c_j.s.logical_len(), 0);
    }
}

fn median_for_logs<R>(fixture: &DonorFixture<R>) -> Vec<Entry> {
    let sequences = fixture.logs.each_ref().map(|log| bare(log));
    median_sequence([
        LabeledSequence::new("A", &sequences[0]),
        LabeledSequence::new("B", &sequences[1]),
        LabeledSequence::new("C", &sequences[2]),
    ])
    .expect("reachable donor logs stay inside the oracle domain")
}

#[test]
fn reachable_donors_define_seed_a_c_as_required_recipient_prefix() {
    let compact: DonorFixture<CompactReply> = compact_donors();
    let recovery: DonorFixture<RecoveryReply> = recovery_donors();

    assert_eq!(median_for_logs(&compact), REQUIRED_PREFIX);
    assert_eq!(median_for_logs(&recovery), REQUIRED_PREFIX);
}

#[test]
fn timestamp_differences_do_not_create_command_order_disagreement() {
    let stamped = [
        vec![
            timed(Entry::Nop(0), 0),
            timed(Entry::Cmd(COMMAND_A), 1),
            timed(Entry::Cmd(COMMAND_C), 2),
        ],
        vec![
            timed(Entry::Nop(0), 0),
            timed(Entry::Cmd(COMMAND_A), 2),
            timed(Entry::Cmd(COMMAND_C), 4),
        ],
        vec![
            timed(Entry::Nop(0), 0),
            timed(Entry::Cmd(COMMAND_A), 3),
            timed(Entry::Cmd(COMMAND_C), 6),
        ],
    ];
    assert_ne!(stamped[0], stamped[1]);
    assert_ne!(stamped[1], stamped[2]);

    let bare_sequences = stamped.each_ref().map(|log| bare(log));
    assert_eq!(
        bare_sequences,
        std::array::from_fn(|_| REQUIRED_PREFIX.to_vec())
    );
    assert_eq!(
        median_sequence([
            LabeledSequence::new("early", &bare_sequences[0]),
            LabeledSequence::new("middle", &bare_sequences[1]),
            LabeledSequence::new("late", &bare_sequences[2]),
        ])
        .expect("identical bare command sequences remain in the oracle domain"),
        REQUIRED_PREFIX
    );
}

#[test]
fn compact_reply_handles_remain_logically_immutable_across_sender_commits() {
    let mut sender = CompactNode::new(Config::default(), 1);
    let held = compact_reply(&sender);
    let genesis = vec![timed(Entry::Nop(0), 0)];
    assert_eq!(held.l_j.entries().to_vec(), genesis);
    assert!(
        held.s_j
            .as_ref()
            .expect("reply carries state")
            .untruncated()
            .is_empty()
    );

    sender.step_chosen(
        &[held.clone(), held.clone(), held.clone()],
        &[(COMMAND_A, 1)],
        1,
        Some(&[0, 1, 2]),
    );
    let current = compact_reply(&sender);
    sender.step_chosen(
        &[current.clone(), current.clone(), current],
        &[],
        2,
        Some(&[0, 1, 2]),
    );

    assert_eq!(held.l_j.entries().to_vec(), genesis);
    assert!(
        held.s_j
            .as_ref()
            .expect("reply carries state")
            .untruncated()
            .is_empty()
    );
    assert_eq!(
        sender.shared_state().untruncated(),
        &[Entry::Cmd(COMMAND_A)]
    );
    assert_eq!(sender.log_entries(), Some(vec![timed(Entry::Nop(2), 2)]));
}

#[test]
fn recovery_reply_handles_remain_logically_immutable_across_boundary_commit() {
    let mut sender = RecoveryNode::new(Config::default(), 1, false);
    let genesis = recovery_reply(&sender);
    sender.step_chosen(
        &[genesis.clone(), genesis.clone(), genesis],
        &[(COMMAND_A, 1)],
        Some(&[0, 1, 2]),
    );
    sender.end_window(1, 2);

    let held = recovery_reply(&sender);
    let pre_committed = vec![timed(Entry::Nop(0), 0), timed(Entry::Cmd(COMMAND_A), 1)];
    assert_eq!(
        held.l_j
            .as_ref()
            .expect("reply carries a log")
            .entries()
            .to_vec(),
        pre_committed
    );
    assert_eq!(held.c_j.w, 1);
    assert_eq!(held.c_j.p.as_deref(), Some(pre_committed.as_slice()));
    assert!(held.c_j.s.untruncated().is_empty());

    sender.end_window(2, 3);

    assert_eq!(
        held.l_j
            .as_ref()
            .expect("reply carries a log")
            .entries()
            .to_vec(),
        pre_committed
    );
    assert_eq!(held.c_j.w, 1);
    assert_eq!(held.c_j.p.as_deref(), Some(pre_committed.as_slice()));
    assert!(held.c_j.s.untruncated().is_empty());
    assert_eq!(
        sender.shared_state().untruncated(),
        &[Entry::Cmd(COMMAND_A)]
    );
    assert_eq!(sender.log_entries(), Some(vec![timed(Entry::Nop(3), 3)]));
}

// Sensitivity check for DET-1 (A5.OPEN.5): under the command-only key the recipient takes the
// command median; the default key reads round stamps and orders this fixture otherwise.
#[test]
fn command_only_median_alternative_compact() {
    let fixture = compact_donors();
    assert_logs_and_uncommitted(&fixture);
    assert_eq!(median_for_logs(&fixture), REQUIRED_PREFIX);

    let recipient_commands = |stamp: StampTie| {
        let mut recipient = CompactNode::new(Config::default(), fixtures::T_NO_COMMIT)
            .with_merge_policy(stamp, UnionStamp::FirstSighting);
        recipient.step_chosen(&fixture.replies, &[], 4, Some(&[0, 1, 2]));
        bare(
            &recipient
                .log_entries()
                .expect("three reachable replies keep the recipient log-bearing"),
        )
    };

    assert_command_only_prefix_and_default_differs(
        &recipient_commands(StampTie::CommandsOnly),
        &recipient_commands(StampTie::IncludeRound),
    );
}

// As above, for Alg 6's inherited extended median rule.
#[test]
fn command_only_median_alternative_recovery() {
    let fixture = recovery_donors();
    assert_logs_and_uncommitted(&fixture);
    assert_eq!(median_for_logs(&fixture), REQUIRED_PREFIX);

    let recipient_commands = |stamp: StampTie| {
        let mut recipient = RecoveryNode::new(Config::default(), fixtures::T_NO_COMMIT, false)
            .with_merge_policy(stamp, UnionStamp::FirstSighting);
        recipient.step_chosen(&fixture.replies, &[], Some(&[0, 1, 2]));
        bare(
            &recipient
                .log_entries()
                .expect("three reachable log replies keep the recipient log-bearing"),
        )
    };

    assert_command_only_prefix_and_default_differs(
        &recipient_commands(StampTie::CommandsOnly),
        &recipient_commands(StampTie::IncludeRound),
    );
}

fn assert_command_only_prefix_and_default_differs(command_only: &[Entry], default: &[Entry]) {
    let required = Some(REQUIRED_PREFIX.as_slice());
    assert_eq!(
        command_only.get(..REQUIRED_PREFIX.len()),
        required,
        "command-median recipient prefix; output {command_only:?}"
    );
    assert_ne!(
        default.get(..REQUIRED_PREFIX.len()),
        required,
        "the fixture no longer separates the two keys; default output {default:?}"
    );
}

#[test]
fn algorithm3_unblocked_bottom_node_amplifies_unseen_command() {
    let mut node = LogNode::new(0, Config::default());
    let mut rng = ChaCha12Rng::seed_from_u64(42);
    node.step(&[], &[], &mut rng);
    assert_eq!(node.log(), None, "below ell replies must establish bottom");

    let wants_amplify = node.wants_amplify(1);
    assert!(
        wants_amplify,
        "Algorithm 3 requires an unblocked bottom-log server to amplify a newly received unseen command; current wants_amplify={wants_amplify}"
    );
}

#[test]
fn algorithm6_bottom_node_inherits_algorithm3_amplification() {
    let mut node = RecoveryNode::new(Config::default(), fixtures::T_NO_COMMIT, false);
    node.step_chosen(&[], &[], None);
    assert_eq!(
        node.log_entries(),
        None,
        "below ell replies must establish bottom"
    );

    let wants_amplify = node.wants_amplify(&fixtures::COMMAND_A);
    assert!(
        wants_amplify,
        "Algorithm 6 inherits Algorithm 3 amplification for an unblocked bottom-log server; current wants_amplify={wants_amplify}"
    );
}

#[test]
fn algorithm6_core_amplification_is_sequence_agnostic() {
    let node = RecoveryNode::new(Config::default(), fixtures::T_NO_COMMIT, false);
    let command = ClientCommand {
        client: 1,
        sn: 2,
        op: 2,
    };

    assert!(
        node.wants_amplify(&command),
        "bare Algorithm 6 inherits bottom-log amplification without an ACK-adapter sequence-number gate"
    );
}
