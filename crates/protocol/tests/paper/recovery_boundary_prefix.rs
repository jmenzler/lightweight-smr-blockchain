use protocol::Config;
use protocol::compact::{ClientCommand, Entry, SharedState, Timed};
use protocol::log::Log;
use protocol::recovery::{Checkpoint, PrefixMismatch, RState, RecoveryNode, RecoveryReply};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use std::panic::{AssertUnwindSafe, catch_unwind};

const T: u64 = 10;

fn cmd(client: u32, sn: u64, op: u64) -> ClientCommand {
    ClientCommand { client, sn, op }
}

fn timed(c: ClientCommand, round: u64) -> Timed {
    Timed {
        entry: Entry::Cmd(c),
        round,
    }
}

fn genesis() -> Checkpoint {
    Checkpoint {
        s: SharedState::default().into(),
        p: None,
        w: 0,
        certs: None,
    }
}

fn reply(log: Option<Vec<Timed>>, checkpoint: Checkpoint) -> RecoveryReply {
    RecoveryReply {
        l_j: log.map(Log::from_entries),
        c_j: checkpoint.into(),
        r_j: RState::NoReset,
    }
}

/// A node holding C_i = (s0, P, 1) whose L_i the Alg 3 merge then replaced with `log`.
fn node_with(pre: Vec<Timed>, log: Vec<Timed>, policy: PrefixMismatch) -> RecoveryNode {
    let cp1 = Checkpoint {
        s: SharedState::default().into(),
        p: Some(pre),
        w: 1,
        certs: None,
    };
    let mut rng = ChaCha12Rng::seed_from_u64(42);
    let mut n = RecoveryNode::new(Config::default(), T, false).with_prefix_mismatch(policy);
    n.step(&vec![reply(None, cp1); 3], &[], 11, &mut rng);
    n.step(
        &vec![reply(Some(log.clone()), genesis()); 3],
        &[],
        12,
        &mut rng,
    );
    assert_eq!(
        n.log_entries(),
        Some(log),
        "fixture: the median of three equal logs is that log"
    );
    n
}

fn commands(entries: &[Timed]) -> Vec<Entry> {
    entries.iter().map(|t| t.entry.clone()).collect()
}

/// B-2a read as written: P is removable iff its commands are the first |P| commands of L_i.
fn command_prefix(pre: &[Timed], log: &[Timed]) -> bool {
    log.len() >= pre.len() && pre.iter().zip(log).all(|(p, l)| p.entry == l.entry)
}

// Alg 6 B-2a, p. 31: P is a sequence of commands; a P whose commands lead L_i is committed and removed.
#[test]
fn boundary_commits_a_command_prefix_whose_round_stamps_differ() {
    let x = cmd(1, 1, 7);
    let y = cmd(2, 1, 8);
    let mut n = node_with(
        vec![timed(x, 10)],
        vec![timed(x, 9), timed(y, 12)],
        PrefixMismatch::Abort,
    );
    let outcome = catch_unwind(AssertUnwindSafe(|| n.end_window(2, 20)));
    assert!(
        outcome.is_ok(),
        "P = [x@10] is a command prefix of L_i = [x@9, y@12]"
    );
    assert_eq!(n.shared_state().untruncated().to_vec(), vec![Entry::Cmd(x)]);
    assert_eq!(
        n.log_entries().expect("non-⊥ after B-2"),
        vec![timed(y, 12)],
        "the kept suffix of L_i keeps L_i's own stamps"
    );
}

// Alg 6 B-2a, p. 31, under the opt-in skip: a stamp-only difference is no mismatch to skip.
#[test]
fn skip_boundary_commits_a_command_prefix_whose_round_stamps_differ() {
    let x = cmd(1, 1, 7);
    let y = cmd(2, 1, 8);
    let mut n = node_with(
        vec![timed(x, 10)],
        vec![timed(x, 9), timed(y, 12)],
        PrefixMismatch::SkipBoundary,
    );
    n.end_window(2, 20);
    assert!(!n.stale_checkpoint(), "the boundary ran");
    assert_eq!(n.checkpoint().w, 2);
    assert_eq!(n.shared_state().untruncated().to_vec(), vec![Entry::Cmd(x)]);
    assert_eq!(
        commands(&n.log_entries().expect("non-⊥")),
        vec![Entry::Cmd(y)]
    );
}

// Alg 6 B-2a, p. 31: the boundary prefix test agrees with the command-level reading on every case.
#[test]
fn boundary_prefix_check_agrees_with_command_level_reference() {
    let a = cmd(1, 1, 1);
    let b = cmd(2, 1, 2);
    let c = cmd(3, 1, 3);
    let cases: Vec<(Vec<Timed>, Vec<Timed>)> = vec![
        (vec![timed(a, 4)], vec![timed(a, 4), timed(b, 6)]),
        (vec![timed(a, 4)], vec![timed(a, 3), timed(b, 6)]),
        (
            vec![timed(a, 4), timed(b, 5)],
            vec![timed(a, 4), timed(b, 7), timed(c, 9)],
        ),
        (
            vec![timed(a, 4), timed(b, 5)],
            vec![timed(a, 2), timed(b, 3)],
        ),
        (vec![timed(a, 4)], vec![timed(b, 4), timed(a, 4)]),
        (
            vec![timed(a, 4), timed(b, 5)],
            vec![timed(a, 4), timed(c, 5)],
        ),
        (vec![timed(a, 4), timed(b, 5)], vec![timed(a, 1)]),
    ];
    for (pre, log) in cases {
        let n = node_with(pre.clone(), log.clone(), PrefixMismatch::Abort);
        assert_eq!(
            n.validate_boundary().is_ok(),
            command_prefix(&pre, &log),
            "P={pre:?} L={log:?}"
        );
    }
}

// Alg 6 B-2a, p. 31: a violation names the first COMMAND mismatch, with both stamped entries.
#[test]
fn a_command_mismatch_is_reported_at_its_first_differing_command() {
    let a = cmd(1, 1, 1);
    let b = cmd(2, 1, 2);
    let c = cmd(3, 1, 3);
    let n = node_with(
        vec![timed(a, 4), timed(b, 5)],
        vec![timed(a, 3), timed(c, 5)],
        PrefixMismatch::Abort,
    );
    let violation = n.validate_boundary().expect_err("b vs c");
    assert_eq!(violation.first_mismatch, 1);
    assert_eq!(violation.expected, Some(timed(b, 5)));
    assert_eq!(violation.actual, Some(timed(c, 5)));
}
