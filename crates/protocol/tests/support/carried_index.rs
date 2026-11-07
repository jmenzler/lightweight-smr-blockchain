//! The merge's containment index, carried on the log instead of rebuilt every
//! round. A node's stored index is what a peer's merge seeds `present` from
//! when it picks that node's log as the median, so at every point a peer could
//! sample it the index must gather the log's own entries in exactly the order
//! a fresh sort would — a stale or unrebased index would hand the merge a
//! containment set that is not the median's.

use protocol::Config;
use protocol::chunked::ChunkSeq;
use protocol::compact::{ClientCommand, CompactNode, CompactReply, Entry, Log, SharedState, Timed};
use protocol::recovery::{Checkpoint, RState, RecoveryNode, RecoveryReply};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use std::sync::Arc;

/// Nothing ages out under this commit delay.
const NO_AGING: u64 = 1_000;

fn rng() -> ChaCha12Rng {
    ChaCha12Rng::seed_from_u64(42)
}

fn timed(entry: Entry, round: u64) -> Timed {
    Timed { entry, round }
}

fn cmd(client: u32, sn: u64, op: u64) -> Entry {
    Entry::Cmd(ClientCommand { client, sn, op })
}

fn reply(entries: &[Timed]) -> CompactReply {
    CompactReply::from_log(ChunkSeq::shared(entries.to_vec()), None)
}

/// The invariant every step leaves behind: an index exactly when there is a
/// log, a permutation of that log's positions, gathering its entries in the
/// order a fresh sort of them would.
fn assert_carried_index(n: &CompactNode, ctx: &str) {
    assert_eq!(
        n.log_perm_arc().is_some(),
        n.log_entries().is_some(),
        "{ctx}: index and log disagree on ⊥"
    );
    let Some(held) = n.log_perm_arc() else {
        return;
    };
    let (log, perm) = (held.entries(), held.perm());
    assert_eq!(
        log.to_vec(),
        n.log_entries().expect("non-⊥"),
        "{ctx}: the index rides a log that is not the node's"
    );
    assert_eq!(perm.len(), log.len(), "{ctx}: index length ≠ log length");
    let mut positions: Vec<u32> = perm.as_ref().clone();
    positions.sort_unstable();
    assert!(
        positions.iter().copied().eq(0..log.len() as u32),
        "{ctx}: index is not a permutation of the log's positions: {perm:?}"
    );
    let gathered: Vec<Entry> = perm
        .iter()
        .map(|&i| {
            log.at(i as usize)
                .expect("perm names a live position")
                .entry
                .clone()
        })
        .collect();
    let mut fresh: Vec<Entry> = log.iter().map(|t| t.entry.clone()).collect();
    fresh.sort_unstable();
    assert_eq!(gathered, fresh, "{ctx}: gather ≠ freshly sorted entries");
}

/// The whole log ages out: the rebase filter empties, the dummy Nop takes
/// its place, and the pushed entry's index must pair with it.
#[test]
fn a_fully_drained_log_carries_the_dummy_nop_with_its_index() {
    let t = 10;
    let mut n = CompactNode::new(Config::default(), t);
    let aged = reply(&[timed(cmd(1, 1, 77), 0)]);
    n.step(&[aged.clone(), aged.clone(), aged], &[], t, &mut rng());
    assert_eq!(
        n.log_entries().expect("non-⊥"),
        [timed(Entry::Nop(t), t)],
        "the emptied log holds exactly the dummy"
    );
    assert_carried_index(&n, "full drain to Nop");
}

fn next(s: &mut u64) -> u64 {
    *s = s
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *s >> 33
}

/// A colliding little universe: three clients, two sequence numbers, three
/// ops, so different commands under one (client, sn) — the merge's ⊥ trigger —
/// turn up often. No `Null` is minted here, which is what lets the drive read
/// a `Null` in a node's log as proof the nulling pass ran.
fn synthetic_entry(s: &mut u64) -> Entry {
    let client = (next(s) % 3) as u32;
    let sn = 1 + next(s) % 2;
    if next(s).is_multiple_of(5) {
        Entry::Nop(next(s) % 3)
    } else {
        cmd(client, sn, next(s) % 3)
    }
}

/// Four nodes sampling each other for sixty rounds, with a commit delay short
/// enough that the aged prefix drains most rounds, conflicting duplicates in
/// the traffic, wire-shaped replies whose index is derived rather than
/// carried, and starved rounds that drop a node to ⊥. Every step of every node
/// must leave the index on its log.
#[test]
fn the_carried_index_survives_a_randomized_multi_round_drive() {
    const NODES: usize = 4;
    const T_COMMIT: u64 = 3;
    let cfg = Config::default();
    let mut rng = ChaCha12Rng::seed_from_u64(9);
    let mut nodes: Vec<CompactNode> = (0..NODES)
        .map(|_| CompactNode::new(cfg, T_COMMIT))
        .collect();
    let mut s: u64 = 20_260_815;
    let (mut saw_bot, mut saw_null, mut saw_commit) = (false, false, false);

    for round in 1..=60u64 {
        // Peers sample the pre-step logs, index and all — the same snapshot
        // the sim's log-request stage takes.
        let snaps: Vec<Option<Log>> = nodes.iter().map(|nd| nd.log_perm_arc().cloned()).collect();
        for (i, node) in nodes.iter_mut().enumerate() {
            let mut replies: Vec<CompactReply> = Vec::new();
            for _ in 0..cfg.k {
                let pick = (next(&mut s) % (NODES as u64 + 1)) as usize;
                if pick == NODES {
                    let len = (next(&mut s) % 4) as usize;
                    let log: Vec<Timed> = (0..len)
                        .map(|_| {
                            let stamp = round.saturating_sub(next(&mut s) % 5);
                            timed(synthetic_entry(&mut s), stamp)
                        })
                        .collect();
                    replies.push(CompactReply::from_log(ChunkSeq::shared(log), None));
                } else if let Some(log) = &snaps[pick] {
                    replies.push(CompactReply {
                        l_j: log.clone(),
                        s_j: None,
                    });
                }
            }
            // Starve the node below ℓ now and then: the ⊥ branch has to keep
            // the pairing too.
            if next(&mut s).is_multiple_of(11) {
                replies.truncate(1);
            }
            let appends: Vec<(ClientCommand, u64)> = (0..next(&mut s) % 3)
                .map(|_| {
                    let client = (next(&mut s) % 3) as u32;
                    let sn = 1 + next(&mut s) % 2;
                    (
                        ClientCommand {
                            client,
                            sn,
                            op: next(&mut s) % 3,
                        },
                        round.saturating_sub(next(&mut s) % 5),
                    )
                })
                .collect();

            node.step(&replies, &appends, round, &mut rng);
            assert_carried_index(node, &format!("round {round}, node {i}"));

            saw_bot |= node.log_entries().is_none();
            saw_null |= node
                .log_entries()
                .is_some_and(|l| l.iter().any(|t| matches!(t.entry, Entry::Null { .. })));
            saw_commit |= node.shared_state().logical_len() > 0;
        }
    }
    assert!(saw_bot, "the drive never reached the ⊥ branch");
    assert!(saw_null, "the drive never nulled a conflicting duplicate");
    assert!(saw_commit, "the drive never drained an aged prefix");
}

/// The aged prefix leaves from the front, so every surviving position moves
/// down by its length: an index left on the pre-drain positions would point
/// past the log's end and gather entries the drain took.
#[test]
fn the_drain_rebases_the_index_onto_the_surviving_suffix() {
    const T_COMMIT: u64 = 10;
    let mut n = CompactNode::new(Config::default(), T_COMMIT);
    // Sorted by entry the log reads 0, 2, 1 — so the rebase has to both drop
    // the drained position and renumber the two that survive.
    let held = [
        timed(cmd(1, 1, 0), 0),            // aged at round T_COMMIT → drains
        timed(cmd(3, 1, 0), T_COMMIT - 1), // young
        timed(cmd(2, 1, 0), T_COMMIT - 1), // young
    ];
    let r = reply(&held);
    n.step(&[r.clone(), r.clone(), r], &[], T_COMMIT, &mut rng());

    let log = n.log_entries().expect("non-⊥");
    assert_eq!(log.len(), 2, "the aged head drained: {log:?}");
    assert_eq!(
        n.shared_state().untruncated(),
        vec![cmd(1, 1, 0)],
        "the drained head executed"
    );
    assert_carried_index(&n, "after the drain");
}

/// Nulling rewrites entries in place, and `Null` sorts after every `Cmd`: an
/// index built before the pass describes an order the log no longer has, so a
/// hit has to rebuild it.
#[test]
fn a_nulled_conflicting_duplicate_rebuilds_the_index() {
    let mut n = CompactNode::new(Config::default(), NO_AGING);
    // Content-lexicographic order is a, b, c, so b is the median and seeds
    // the merge; a and c each contribute one fresh entry, and a's collides
    // with b's head under (client 1, sn 1).
    let a = reply(&[timed(cmd(1, 1, 7), 0)]);
    let b = reply(&[timed(cmd(1, 1, 8), 0), timed(cmd(2, 1, 5), 0)]);
    let c = reply(&[timed(cmd(3, 1, 1), 0)]);
    n.step(&[a, b, c], &[], 0, &mut rng());

    let log = n.log_entries().expect("non-⊥");
    assert_eq!(
        log.iter()
            .filter(|t| matches!(t.entry, Entry::Null { client: 1, sn: 1 }))
            .count(),
        2,
        "both conflicting commands became ⊥: {log:?}"
    );
    assert!(
        log.iter().any(|t| t.entry == cmd(2, 1, 5)),
        "the non-conflicting commands survive: {log:?}"
    );
    assert_carried_index(&n, "after the nulling");
}

// ── the recovery rule's index ────────────────────────────────────────────
//
// Algorithm 6 carries the same index, and has three places the compact rule
// does not: the boundary strip that commits P out of the log, the dummy Nop
// that fills an emptied one, and the two paths that replace the log wholesale
// from a checkpoint (max-W adoption, and the reset rollback). Each rebuilds or
// rebases the index, and a log-only test would see none of them.

/// The same invariant as `assert_carried_index`, on the recovery node.
fn assert_recovery_index(n: &RecoveryNode, ctx: &str) {
    assert_eq!(
        n.log_perm_arc().is_some(),
        n.log_entries().is_some(),
        "{ctx}: index and log disagree on ⊥"
    );
    let Some(held) = n.log_perm_arc() else {
        return;
    };
    let (log, perm) = (held.entries(), held.perm());
    assert_eq!(
        log.to_vec(),
        n.log_entries().expect("non-⊥"),
        "{ctx}: the index rides a log that is not the node's"
    );
    assert_eq!(perm.len(), log.len(), "{ctx}: index length ≠ log length");
    let mut positions: Vec<u32> = perm.as_ref().clone();
    positions.sort_unstable();
    assert!(
        positions.iter().copied().eq(0..log.len() as u32),
        "{ctx}: index is not a permutation of the log's positions: {perm:?}"
    );
    let gathered: Vec<Entry> = perm
        .iter()
        .map(|&i| {
            log.at(i as usize)
                .expect("perm names a live position")
                .entry
                .clone()
        })
        .collect();
    let mut fresh: Vec<Entry> = log.iter().map(|t| t.entry.clone()).collect();
    fresh.sort_unstable();
    assert_eq!(gathered, fresh, "{ctx}: gather ≠ freshly sorted entries");
}

const T_WINDOW: u64 = 5;

fn checkpoint(pre: Option<Vec<Timed>>, window: u64) -> Arc<Checkpoint> {
    Arc::new(Checkpoint {
        s: SharedState::default().into(),
        p: pre,
        w: window,
        certs: None,
    })
}

fn rec_reply(log: Option<Vec<Timed>>, cp: &Arc<Checkpoint>, r: RState) -> RecoveryReply {
    RecoveryReply {
        l_j: log.map(Log::from_entries),
        c_j: Arc::clone(cp),
        r_j: r,
    }
}

/// Entries deliberately out of sorted order, so an index quietly left as the
/// identity permutation fails the gather.
fn unsorted_log() -> Vec<Timed> {
    vec![
        timed(cmd(3, 1, 30), 0),
        timed(cmd(1, 1, 10), 0),
        timed(cmd(2, 1, 20), 0),
    ]
}

/// Max-W adoption replaces the log wholesale with the checkpoint's P, which
/// carries no index of its own — so one has to be built for it, and it must be
/// the adopted log's, never the discarded merge's.
#[test]
fn an_adopted_checkpoint_log_arrives_with_a_rebuilt_index() {
    let p = unsorted_log();
    let ahead = checkpoint(Some(p.clone()), 1);
    let mut n = RecoveryNode::new(Config::default(), T_WINDOW, false);
    // No reply carries a log, so the node's own merge goes ⊥ and adoption is
    // the only thing that can put a log back.
    n.step(
        &vec![rec_reply(None, &ahead, RState::NoReset); 3],
        &[],
        1,
        &mut rng(),
    );
    assert_eq!(n.log_entries().expect("adopted"), &p[..]);
    assert_recovery_index(&n, "after adoption");
}

/// The boundary commits P and strips it out of the log. Every surviving place
/// moves down by the stripped length, and the index has to move with it.
#[test]
fn the_boundary_strip_rebases_the_recovery_index() {
    let p = unsorted_log();
    let ahead = checkpoint(Some(p.clone()), 1);
    let mut n = RecoveryNode::new(Config::default(), T_WINDOW, false);
    n.step(
        &vec![rec_reply(None, &ahead, RState::NoReset); 3],
        &[],
        1,
        &mut rng(),
    );

    // A merge over logs that all extend P keeps P as the merged log's prefix,
    // which is what the strip requires.
    let mut ext = p.clone();
    ext.push(timed(cmd(0, 1, 5), 2));
    ext.push(timed(cmd(4, 1, 40), 2));
    n.step(
        &vec![rec_reply(Some(ext.clone()), &ahead, RState::NoReset); 3],
        &[],
        2,
        &mut rng(),
    );
    assert_eq!(n.log_entries().expect("merged"), &ext[..]);
    assert_recovery_index(&n, "after the merge");

    n.end_window(2, T_WINDOW);
    assert_eq!(
        n.log_entries().expect("non-⊥"),
        &ext[p.len()..],
        "the committed prefix left the log"
    );
    assert_recovery_index(&n, "after the boundary strip");
}

/// The whole log commits out: the rebase empties, the dummy Nop takes its
/// place, and the pushed entry's index must pair with it.
#[test]
fn a_fully_stripped_recovery_log_carries_the_dummy_nop_with_its_index() {
    let p = unsorted_log();
    let ahead = checkpoint(Some(p.clone()), 1);
    let mut n = RecoveryNode::new(Config::default(), T_WINDOW, false);
    n.step(
        &vec![rec_reply(None, &ahead, RState::NoReset); 3],
        &[],
        1,
        &mut rng(),
    );

    let round = 7;
    n.end_window(2, round);
    assert_eq!(
        n.log_entries().expect("non-⊥"),
        [timed(Entry::Nop(round), round)],
        "the emptied log holds exactly the dummy"
    );
    assert_recovery_index(&n, "full strip to dummy");
}

/// The reset rollback restores the log from the checkpoint before the strip
/// runs, so its index is rebuilt on a path the merge never touches.
#[test]
fn the_reset_rollback_rebuilds_the_recovery_index() {
    let p = unsorted_log();
    let ahead = checkpoint(Some(p.clone()), 1);
    let mut n = RecoveryNode::new(Config::default(), T_WINDOW, false);
    n.step(
        &vec![rec_reply(None, &ahead, RState::NoReset); 3],
        &[],
        1,
        &mut rng(),
    );

    // Every reply votes reset and none carries a log: the node drops to a ⊥
    // log with R = Reset, which is the state the boundary rolls back from.
    n.step(
        &vec![rec_reply(None, &ahead, RState::Reset); 3],
        &[],
        2,
        &mut rng(),
    );
    assert!(n.log_entries().is_none(), "no log-bearing reply → ⊥");
    assert_eq!(n.reset_state(), RState::Reset);

    let round = 9;
    n.end_window(2, round);
    // Rollback restores P, then the same boundary commits it — so the log the
    // rollback rebuilt an index for is stripped to the dummy in one action.
    assert_eq!(
        n.log_entries().expect("rolled back, not ⊥"),
        [timed(Entry::Nop(round), round)]
    );
    assert_recovery_index(&n, "after the reset rollback");
}

/// Four recovery nodes sampling each other for forty in-window rounds, with
/// starved rounds that drop a node to ⊥. Every step of every node must leave
/// the index on its log.
#[test]
fn the_recovery_index_survives_a_randomized_multi_round_drive() {
    const NODES: usize = 4;
    let cfg = Config::default();
    let mut rng = ChaCha12Rng::seed_from_u64(23);
    let mut nodes: Vec<RecoveryNode> = (0..NODES)
        .map(|_| RecoveryNode::new(cfg, T_WINDOW, false))
        .collect();
    let mut s: u64 = 20_260_816;
    let (mut saw_bot, mut saw_growth) = (false, false);

    for round in 1..=40u64 {
        let snaps: Vec<Option<Log>> = nodes.iter().map(|nd| nd.log_perm_arc().cloned()).collect();
        let cps: Vec<Arc<Checkpoint>> = nodes.iter().map(|nd| nd.checkpoint_shared()).collect();
        for (i, node) in nodes.iter_mut().enumerate() {
            let mut replies: Vec<RecoveryReply> = Vec::new();
            for _ in 0..cfg.k {
                let pick = (next(&mut s) % NODES as u64) as usize;
                replies.push(RecoveryReply {
                    l_j: snaps[pick].clone(),
                    c_j: Arc::clone(&cps[pick]),
                    r_j: RState::NoReset,
                });
            }
            // Starve the node below ℓ now and then: the ⊥ branch has to keep
            // the pairing too.
            if next(&mut s).is_multiple_of(9) {
                replies.truncate(1);
            }
            let appends: Vec<(ClientCommand, u64)> = (0..next(&mut s) % 3)
                .map(|_| {
                    let client = (next(&mut s) % 3) as u32;
                    let sn = 1 + next(&mut s) % 2;
                    (
                        ClientCommand {
                            client,
                            sn,
                            op: next(&mut s) % 3,
                        },
                        round.saturating_sub(next(&mut s) % 5),
                    )
                })
                .collect();

            node.step(&replies, &appends, round, &mut rng);
            assert_recovery_index(node, &format!("round {round}, node {i}"));

            saw_bot |= node.log_entries().is_none();
            saw_growth |= node.log_entries().is_some_and(|l| l.len() > 3);
        }
    }
    assert!(saw_bot, "the drive never reached the ⊥ branch");
    assert!(saw_growth, "the drive never grew a log past the seed");
}

#[test]
#[should_panic(expected = "carried index must name every position")]
fn a_carried_index_shorter_than_its_log_is_refused() {
    let log = ChunkSeq::from_vec(vec![timed(Entry::Nop(0), 0), timed(Entry::Nop(1), 1)]);
    protocol::merge::PermIndex::from_carried(&log, &[0]);
}

#[test]
#[should_panic(expected = "cannot remove 2 of 1 entries")]
fn removing_more_entries_than_the_merge_holds_is_refused() {
    use protocol::merge::{StampTie, UnionStamp, median_merge};
    let log = protocol::log::Log::from_entries(vec![timed(Entry::Nop(0), 0)]);
    let merged = median_merge(
        std::iter::repeat_n(&log, 3),
        3,
        &[],
        StampTie::default(),
        UnionStamp::default(),
    );
    merged.remove_prefix(2, timed(Entry::Nop(5), 5));
}
