//! Guards for the reply payloads that are shared rather than copied. Sharing
//! must not move a single observable: the median sort still orders logs by
//! their CONTENT (stamp rounds included, not just entries, and never by any
//! wrapper's own identity), the merge still names a command once however many
//! non-median logs carry it, and a ⊥ log stays distinct from an empty one.

use protocol::Config;
use protocol::chunked::ChunkSeq;
use protocol::compact::{ClientCommand, CompactNode, CompactReply, Entry, SharedState, Timed};
use protocol::log::Log;
use protocol::merge::{StampTie, UnionStamp};
use protocol::recovery::{Checkpoint, RState, RecoveryNode, RecoveryReply};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use std::collections::BTreeMap;
use std::sync::Arc;

const NO_AGING: u64 = 1_000;

fn rng() -> ChaCha12Rng {
    ChaCha12Rng::seed_from_u64(42)
}

fn cmd(client: u32, op: u64) -> Entry {
    Entry::Cmd(ClientCommand { client, sn: 1, op })
}

fn timed(entry: Entry, round: u64) -> Timed {
    Timed { entry, round }
}

fn genesis() -> Checkpoint {
    Checkpoint {
        s: SharedState::default().into(),
        p: None,
        w: 0,
        certs: None,
    }
}

fn compact_replies(logs: Vec<Vec<Timed>>) -> Vec<CompactReply> {
    logs.into_iter()
        .map(|log| CompactReply::from_log(ChunkSeq::shared(log), None))
        .collect()
}

fn compact_merge(logs: Vec<Vec<Timed>>) -> Vec<Timed> {
    let replies = compact_replies(logs);
    let mut n = CompactNode::new(Config::default(), NO_AGING);
    n.step(&replies, &[], 0, &mut rng());
    n.log_entries().expect("non-⊥").to_vec()
}

fn compact_merge_with(logs: Vec<Vec<Timed>>, stamp: StampTie, union: UnionStamp) -> Vec<Timed> {
    let replies = compact_replies(logs);
    let chosen: Vec<usize> = (0..replies.len()).collect();
    let mut n = CompactNode::new(Config::default(), NO_AGING).with_merge_policy(stamp, union);
    n.step_chosen(&replies, &[], 0, Some(&chosen));
    n.log_entries().expect("non-⊥").to_vec()
}

fn recovery_replies(logs: Vec<Option<Vec<Timed>>>) -> Vec<RecoveryReply> {
    logs.into_iter()
        .map(|log| RecoveryReply {
            l_j: log.map(Log::from_entries),
            c_j: genesis().into(),
            r_j: RState::NoReset,
        })
        .collect()
}

fn recovery_merge(logs: Vec<Option<Vec<Timed>>>) -> Option<Vec<Timed>> {
    let replies = recovery_replies(logs);
    let mut n = RecoveryNode::new(Config::default(), NO_AGING, false);
    n.step(&replies, &[], 0, &mut rng());
    n.log_entries()
}

fn recovery_merge_with(
    logs: Vec<Option<Vec<Timed>>>,
    stamp: StampTie,
    union: UnionStamp,
) -> Option<Vec<Timed>> {
    let replies = recovery_replies(logs);
    let chosen: Vec<usize> = (0..replies.len()).collect();
    let mut n =
        RecoveryNode::new(Config::default(), NO_AGING, false).with_merge_policy(stamp, union);
    n.step_chosen(&replies, &[], Some(&chosen));
    n.log_entries()
}

/// Three logs that differ ONLY in their stamp rounds: `Timed`'s order runs
/// (entry, round), so the median is the middle round. A comparator that
/// dropped the round — or ordered by a wrapper instead of the content —
/// would pick a different log and this pins it.
#[test]
fn the_median_sort_orders_by_content_including_stamp_rounds() {
    let picked = compact_merge(vec![
        vec![timed(cmd(1, 1), 9)],
        vec![timed(cmd(1, 1), 2)],
        vec![timed(cmd(1, 1), 5)],
    ]);
    assert_eq!(picked, vec![timed(cmd(1, 1), 5)]);
}

/// The same pin one level up: entries decide before rounds do. The median
/// leads the merge, so its head names which log was picked.
#[test]
fn the_median_sort_orders_by_entry_before_round() {
    let merged = compact_merge(vec![
        vec![timed(cmd(3, 3), 0)],
        vec![timed(cmd(1, 1), 99)],
        vec![timed(cmd(2, 2), 0)],
    ]);
    assert_eq!(
        merged,
        vec![
            timed(cmd(2, 2), 0),
            timed(cmd(1, 1), 99),
            timed(cmd(3, 3), 0)
        ]
    );
}

/// K*: a command carried by two non-median logs enters the merge once.
#[test]
fn a_command_in_two_non_median_logs_appears_once() {
    let merged = compact_merge(vec![
        vec![timed(cmd(1, 1), 0), timed(cmd(9, 9), 7)],
        vec![timed(cmd(2, 2), 0)],
        vec![timed(cmd(3, 3), 0), timed(cmd(9, 9), 8)],
    ]);
    let nines = merged.iter().filter(|t| t.entry == cmd(9, 9)).count();
    assert_eq!(nines, 1, "merged: {merged:?}");
    // The first sighting wins, stamp and all — the later duplicate is dropped
    // rather than overwriting it.
    assert_eq!(merged[0], timed(cmd(2, 2), 0), "median leads the merge");
    assert!(merged.contains(&timed(cmd(9, 9), 7)));
}

/// The paper leaves the lex-order key open; both readings are admissible. Under the
/// commands-only reading three logs that differ only in stamp are a tie. Stable
/// sample order, identity permutation: the middle input wins, not the middle stamp.
#[test]
fn commands_only_median_ignores_stamp_rounds() {
    let picked = compact_merge_with(
        vec![
            vec![timed(cmd(1, 1), 9)],
            vec![timed(cmd(1, 1), 2)],
            vec![timed(cmd(1, 1), 5)],
        ],
        StampTie::CommandsOnly,
        UnionStamp::FirstSighting,
    );
    assert_eq!(picked, vec![timed(cmd(1, 1), 2)]);
}

/// Diagnostic overwrite policy: a later walk of the same command replaces
/// its stamp. After IncludeRound sorting, cmd9@8 is the later sighting.
#[test]
fn last_sighting_keeps_the_later_walk_stamp() {
    let merged = compact_merge_with(
        vec![
            vec![timed(cmd(1, 1), 0), timed(cmd(9, 9), 7)],
            vec![timed(cmd(2, 2), 0)],
            vec![timed(cmd(3, 3), 0), timed(cmd(9, 9), 8)],
        ],
        StampTie::IncludeRound,
        UnionStamp::LastSighting,
    );
    let nines: Vec<_> = merged
        .iter()
        .filter(|t| t.entry == cmd(9, 9))
        .cloned()
        .collect();
    assert_eq!(nines, vec![timed(cmd(9, 9), 8)]);
}

#[test]
fn recovery_commands_only_median_ignores_stamp_rounds() {
    let picked = recovery_merge_with(
        vec![
            Some(vec![timed(cmd(1, 1), 9)]),
            Some(vec![timed(cmd(1, 1), 2)]),
            Some(vec![timed(cmd(1, 1), 5)]),
        ],
        StampTie::CommandsOnly,
        UnionStamp::FirstSighting,
    )
    .expect("non-⊥");
    assert_eq!(picked, vec![timed(cmd(1, 1), 2)]);
}

/// Alg 5 takes the median L'_i whole, stamps included: an append re-sighting a median entry leaves it.
#[test]
fn last_sighting_leaves_a_median_entrys_stamp_to_an_append() {
    let x = ClientCommand {
        client: 9,
        sn: 1,
        op: 9,
    };
    let replies = compact_replies(vec![
        vec![timed(Entry::Nop(0), 0), timed(Entry::Cmd(x), 1)];
        3
    ]);
    let mut n = CompactNode::new(Config::default(), NO_AGING)
        .with_merge_policy(StampTie::IncludeRound, UnionStamp::LastSighting);
    n.step_chosen(&replies, &[(x, 5)], 5, Some(&[0, 1, 2]));
    let stamp = n
        .log_entries()
        .expect("non-⊥")
        .into_iter()
        .find(|t| t.entry == Entry::Cmd(x))
        .map(|t| t.round);
    assert_eq!(stamp, Some(1));
}

#[test]
fn recovery_last_sighting_keeps_the_later_walk_stamp() {
    let merged = recovery_merge_with(
        vec![
            Some(vec![timed(cmd(1, 1), 0), timed(cmd(9, 9), 7)]),
            Some(vec![timed(cmd(2, 2), 0)]),
            Some(vec![timed(cmd(3, 3), 0), timed(cmd(9, 9), 8)]),
        ],
        StampTie::IncludeRound,
        UnionStamp::LastSighting,
    )
    .expect("non-⊥");
    let nines: Vec<_> = merged
        .iter()
        .filter(|t| t.entry == cmd(9, 9))
        .cloned()
        .collect();
    assert_eq!(nines, vec![timed(cmd(9, 9), 8)]);
}

/// Same pin on the recovery merge, whose logs arrive optional.
#[test]
fn the_recovery_merge_also_names_a_shared_command_once() {
    let merged = recovery_merge(vec![
        Some(vec![timed(cmd(1, 1), 0), timed(cmd(9, 9), 7)]),
        Some(vec![timed(cmd(2, 2), 0)]),
        Some(vec![timed(cmd(3, 3), 0), timed(cmd(9, 9), 8)]),
    ])
    .expect("non-⊥");
    assert_eq!(merged.iter().filter(|t| t.entry == cmd(9, 9)).count(), 1);
}

/// The stored log is shared, never copied: a snapshot Arc taken before a
/// step still reads the pre-step content after the node swaps its log in.
#[test]
fn a_held_log_snapshot_survives_the_next_step_unchanged() {
    let mut n = CompactNode::new(Config::default(), NO_AGING);
    n.step(
        &vec![CompactReply::from_log(ChunkSeq::shared(vec![timed(cmd(1, 1), 0)]), None); 3],
        &[],
        0,
        &mut rng(),
    );
    let snap = std::sync::Arc::clone(n.log_arc().expect("non-⊥"));
    let before: Vec<Timed> = snap.to_vec();
    n.step(
        &vec![CompactReply::from_log(ChunkSeq::shared(vec![timed(cmd(2, 2), 1)]), None); 3],
        &[],
        1,
        &mut rng(),
    );
    assert_eq!(snap.to_vec(), before, "held snapshot moved with the node");
    assert_ne!(
        n.log_entries().expect("non-⊥"),
        before.as_slice(),
        "the node's own log did move"
    );
}

/// `log_arc` and `log_entries` are the same view: ⊥ on both or neither.
#[test]
fn log_arc_mirrors_log_entries_on_bot() {
    let mut n = CompactNode::new(Config::default(), NO_AGING);
    assert!(n.log_arc().is_some());
    assert_eq!(
        n.log_arc().map(|l| l.to_vec()),
        n.log_entries(),
        "fresh node: same view"
    );
    n.step(&[], &[], 0, &mut rng());
    assert!(n.log_arc().is_none(), "below ℓ the node is ⊥ on both views");
    assert!(n.log_entries().is_none());
}

/// The boundary strip under a shared Arc equals the unshared run: holding a
/// snapshot across `end_window` must not change the stripped log or the
/// minted checkpoint.
#[test]
fn a_shared_log_does_not_change_the_boundary_strip() {
    let drive = |hold: bool| {
        let mut n = RecoveryNode::new(Config::default(), 1, false);
        n.step(
            &vec![
                RecoveryReply {
                    l_j: Some(Log::from_entries(vec![
                        timed(cmd(1, 1), 0),
                        timed(cmd(2, 2), 0)
                    ])),
                    c_j: genesis().into(),
                    r_j: RState::NoReset,
                };
                3
            ],
            &[],
            0,
            &mut rng(),
        );
        let held = hold.then(|| n.log_arc().map(std::sync::Arc::clone));
        let held_before: Option<Vec<Timed>> =
            held.as_ref().and_then(|h| h.as_deref().map(|l| l.to_vec()));
        n.end_window(1, 100);
        let held2 = hold.then(|| n.log_arc().map(std::sync::Arc::clone));
        let held2_before: Option<Vec<Timed>> = held2
            .as_ref()
            .and_then(|h| h.as_deref().map(|l| l.to_vec()));
        n.end_window(2, 200);
        if hold {
            // The strip must never reach a held snapshot: both handles still
            // read their pre-boundary content.
            assert_eq!(
                held.as_ref().unwrap().as_deref().map(|l| l.to_vec()),
                held_before,
                "first held snapshot moved with the strip"
            );
            assert_eq!(
                held2.as_ref().unwrap().as_deref().map(|l| l.to_vec()),
                held2_before,
                "second held snapshot moved with the strip"
            );
        }
        drop((held, held2));
        (n.log_entries(), n.checkpoint().p.clone(), n.checkpoint().w)
    };
    assert_eq!(drive(true), drive(false));
}

/// ⊥ is not the empty log: replies without one do not reach the ℓ-count, so
/// the node goes ⊥ instead of merging what did arrive.
#[test]
fn bot_reply_logs_stay_distinct_from_empty_ones() {
    assert_eq!(
        recovery_merge(vec![Some(vec![timed(cmd(1, 1), 0)]), None, None]),
        None,
        "two ⊥ logs leave fewer than ℓ log-bearing replies"
    );
    assert_eq!(
        recovery_merge(vec![Some(vec![]), Some(vec![]), Some(vec![])]),
        Some(vec![]),
        "three empty logs are a merge, not a ⊥"
    );
}

// --- the state is a shared handle: adoption, mint and rollback share the
// --- allocation, and every commit copies before writing

fn donor_state() -> Arc<SharedState> {
    Arc::new(SharedState::from_entries(
        vec![cmd(7, 7)],
        BTreeMap::from([(7, 1)]),
    ))
}

#[test]
fn a_bot_compact_node_adopts_the_reply_state_by_handle() {
    let mut n = CompactNode::new(Config::default(), NO_AGING);
    n.step_chosen(&[], &[], 0, None);
    assert!(n.b_i());
    let donor = donor_state();
    let reply = CompactReply::from_log(
        ChunkSeq::shared(vec![timed(cmd(1, 1), 0)]),
        Some(Arc::clone(&donor)),
    );
    n.step_chosen(&[reply], &[], 1, None);
    assert!(
        Arc::ptr_eq(n.state_arc(), &donor),
        "sub-ell adoption must share the donor allocation, not copy it"
    );
}

#[test]
fn a_bot_compact_node_adopts_through_the_merge_by_handle() {
    let mut n = CompactNode::new(Config::default(), NO_AGING);
    n.step_chosen(&[], &[], 0, None);
    let donor = donor_state();
    let replies: Vec<CompactReply> = (0..3)
        .map(|_| {
            CompactReply::from_log(
                ChunkSeq::shared(vec![timed(cmd(1, 1), 0)]),
                Some(Arc::clone(&donor)),
            )
        })
        .collect();
    n.step_chosen(&replies, &[], 1, Some(&[0, 1, 2]));
    assert!(
        Arc::ptr_eq(n.state_arc(), &donor),
        "state adoption on the ≥ ℓ path must share the donor allocation, not copy it"
    );
}

#[test]
fn an_adopters_first_commit_copies_instead_of_touching_the_donor() {
    let mut n = CompactNode::new(Config::default(), 1);
    n.step_chosen(&[], &[], 0, None);
    let donor = donor_state();
    let reply = CompactReply::from_log(
        ChunkSeq::shared(vec![timed(cmd(1, 1), 0)]),
        Some(Arc::clone(&donor)),
    );
    // Round 5: the reply's entry is aged, so adoption is followed by a commit
    // in the same step — which must copy, never write through the shared handle.
    n.step_chosen(&[reply], &[], 5, None);
    assert!(!Arc::ptr_eq(n.state_arc(), &donor));
    assert_eq!(donor.logical_len(), 1, "the donor state is untouched");
    assert_eq!(donor.sn_get(1), None);
    assert_eq!(n.shared_state().logical_len(), 2);
    assert_eq!(n.shared_state().sn_get(1), Some(1));
}

#[test]
fn the_boundary_mint_shares_the_node_state_by_handle() {
    let mut n = RecoveryNode::new(Config::default(), 1, false);
    n.end_window(1, 10);
    assert!(
        Arc::ptr_eq(n.state_arc(), &n.checkpoint().s),
        "the mint must hand the checkpoint the node's own handle"
    );
}

#[test]
fn checkpoint_adoption_installs_the_state_by_handle() {
    let mut n = RecoveryNode::new(Config::default(), 1, false);
    let donor = donor_state();
    let cp = Arc::new(Checkpoint {
        s: Arc::clone(&donor),
        p: Some(vec![timed(cmd(1, 1), 0)]),
        w: 5,
        certs: None,
    });
    let replies: Vec<RecoveryReply> = (0..3)
        .map(|_| RecoveryReply {
            l_j: None,
            c_j: Arc::clone(&cp),
            r_j: RState::NoReset,
        })
        .collect();
    n.step_chosen(&replies, &[], None);
    assert!(
        Arc::ptr_eq(n.state_arc(), &donor),
        "adoption must install the checkpoint's state by handle"
    );
}

#[test]
fn a_rollback_boundary_copies_before_committing_over_the_checkpoint() {
    let mut n = RecoveryNode::new(Config::default(), 1, false);
    let held = donor_state();
    let cp = Arc::new(Checkpoint {
        s: Arc::clone(&held),
        p: Some(vec![timed(cmd(1, 1), 0)]),
        w: 5,
        certs: None,
    });
    let replies: Vec<RecoveryReply> = (0..3)
        .map(|_| RecoveryReply {
            l_j: None,
            c_j: Arc::clone(&cp),
            r_j: RState::Reset,
        })
        .collect();
    n.step_chosen(&replies, &[], None);
    assert_eq!(n.reset_state(), RState::Reset);
    // Rollback re-installs the checkpoint's handle; the boundary commit that
    // follows sixteen lines later must copy before extending it.
    n.end_window(6, 50);
    assert_eq!(held.logical_len(), 1, "the checkpoint state is untouched");
    assert_eq!(held.sn_get(1), None);
    assert_eq!(n.shared_state().logical_len(), 2);
    assert_eq!(n.shared_state().sn_get(1), Some(1));
}

// --- boundary interning: a shared mint's member holds the class state too,
// --- proven by the false-returning comparator, never PartialEq

/// Both nodes commit the same entry independently, then take two boundaries —
/// the member through the shared-mint path. After the second boundary the
/// member's state must BE the class checkpoint's allocation.
fn committed_pair() -> (RecoveryNode, RecoveryNode) {
    let mk = || {
        let mut n = RecoveryNode::new(Config::default(), 1, false);
        let log = vec![timed(cmd(4, 4), 0)];
        let replies: Vec<RecoveryReply> = (0..3)
            .map(|_| RecoveryReply {
                l_j: Some(Log::from_entries(log.clone())),
                c_j: genesis().into(),
                r_j: RState::NoReset,
            })
            .collect();
        n.step_chosen(&replies, &[], Some(&[0, 1, 2]));
        n
    };
    let (mut rep, mut member) = (mk(), mk());
    rep.end_window(1, 10);
    member.end_window_shared(1, 10, &rep.checkpoint_shared());
    (rep, member)
}

#[test]
fn a_shared_boundary_interns_the_members_state() {
    let (mut rep, mut member) = committed_pair();
    rep.end_window(2, 20);
    member.end_window_shared(2, 20, &rep.checkpoint_shared());
    assert_eq!(member.shared_state().logical_len(), 1);
    assert_eq!(member.shared_state().sn_get(4), Some(1));
    assert!(
        Arc::ptr_eq(member.state_arc(), &rep.checkpoint().s),
        "the member's state must be the class checkpoint's allocation"
    );
}

/// The comparator trap, pinned: a member whose forget line ran ahead reaches
/// the share with an equal-length state and a DIFFERENT offset — the exact
/// pair `SharedState`'s PartialEq asserts on. The interning comparator must
/// answer false and leave the member's state alone; asserting here would
/// panic a lean release run.
#[test]
fn an_offset_mismatch_skips_the_share_instead_of_panicking() {
    let (mut rep, mut member) = committed_pair();
    rep.end_window(2, 20);
    member.end_window_shared(2, 20, &rep.checkpoint_shared());
    member.forget_committed_prefix(1);
    assert_eq!(member.shared_state().executed_offset(), 1);
    rep.end_window(3, 30);
    member.end_window_shared(3, 30, &rep.checkpoint_shared());
    assert!(
        !Arc::ptr_eq(member.state_arc(), &rep.checkpoint().s),
        "an offset mismatch must skip the share"
    );
    assert_eq!(
        member.shared_state().executed_offset(),
        1,
        "state untouched"
    );
    assert_eq!(member.shared_state().logical_len(), 1);
}
