use protocol::Config;
use protocol::compact::{ClientCommand, Entry, SharedState, Triage};
use protocol::recovery::{Checkpoint, RState, RecoveryNode, RecoveryReply};
use std::sync::Arc;

fn cmd(client: u32, sn: u64) -> ClientCommand {
    ClientCommand { client, sn, op: sn }
}

/// A node that adopted a checkpoint whose state already committed `sn` for `client`.
fn node_with_committed(client: u32, sn: u64) -> RecoveryNode {
    let mut node = RecoveryNode::new(Config::default(), 8, false);
    let adopted = RecoveryReply {
        l_j: None,
        c_j: Arc::new(Checkpoint {
            s: Arc::new(SharedState::from_entries(Vec::new(), [(client, sn)].into())),
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
    node
}

// Alg 5, p. 26, subsumed in §6 (p. 30): sn(x) = sn(c) + 1 and x ∉ L_i -> amplify.
#[test]
fn the_next_sequence_number_is_amplified() {
    assert_eq!(
        node_with_committed(1, 2).on_client_command(&cmd(1, 3)),
        Triage::Amplify
    );
}

// Alg 5, p. 26: sn(x) = sn(c) -> inform the client that x is committed.
#[test]
fn the_committed_sequence_number_is_acknowledged() {
    assert_eq!(
        node_with_committed(1, 2).on_client_command(&cmd(1, 2)),
        Triage::AckCommitted
    );
}

// Alg 5, p. 26: "otherwise, i ignores x" — a stale resend must not be spread again.
#[test]
fn a_stale_sequence_number_is_ignored() {
    assert_eq!(
        node_with_committed(1, 2).on_client_command(&cmd(1, 1)),
        Triage::Ignore
    );
}

// Alg 5, p. 26: a gap in sequence numbers is ignored.
#[test]
fn a_future_sequence_number_is_ignored() {
    assert_eq!(
        node_with_committed(1, 2).on_client_command(&cmd(1, 4)),
        Triage::Ignore
    );
}

// Alg 5, p. 26: x ∈ L_i is not amplified again.
#[test]
fn a_command_already_in_the_log_is_ignored() {
    let node = RecoveryNode::new(Config::default(), 8, false);
    let x = cmd(1, 1);
    let reply = node.answer().expect("a fresh node answers");
    let mut node = node;
    node.step_chosen(
        &[reply.clone(), reply.clone(), reply],
        &[(x, 1)],
        Some(&[0, 1, 2]),
    );
    assert!(
        node.log_perm_arc()
            .is_some_and(|l| l.entries().iter().any(|t| t.entry == Entry::Cmd(x)))
    );
    assert_eq!(node.on_client_command(&x), Triage::Ignore);
}

// Alg 5, p. 26: every branch requires L_i ≠ ⊥.
#[test]
fn a_bottom_log_ignores_every_command() {
    let mut node = RecoveryNode::new(Config::default(), 8, false);
    node.step_chosen(&[], &[], None);
    assert!(node.log_perm_arc().is_none());
    for sn in 0..3 {
        assert_eq!(node.on_client_command(&cmd(1, sn)), Triage::Ignore);
    }
}
