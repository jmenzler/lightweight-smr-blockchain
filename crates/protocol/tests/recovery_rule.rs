use protocol::Config;
use protocol::certificates::{MmrForest, ServerCertState, leaf_hash};
use protocol::compact::{ClientCommand, Entry, OutOfSnOrder, RepeatedCommit, SharedState, Timed};
use protocol::log::Log;
use protocol::recovery::{
    Checkpoint, PrefixMismatch, RState, RecoveryNode, RecoveryReply, latest_checkpoint_index,
};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use std::sync::Arc;

const T: u64 = 10;

fn rng() -> ChaCha12Rng {
    ChaCha12Rng::seed_from_u64(42)
}

fn node() -> RecoveryNode {
    RecoveryNode::new(Config::default(), T, true)
}

/// Every node and checkpoint this file builds mounts §5, so the cert reads
/// below assert their own premise rather than testing an empty layer.
fn held(cp: &Checkpoint) -> &ServerCertState {
    cp.certs.as_ref().expect("this checkpoint mounts §5")
}

fn held_mut(cp: &mut Checkpoint) -> &mut ServerCertState {
    cp.certs.as_mut().expect("this checkpoint mounts §5")
}

fn certs(n: &RecoveryNode) -> &ServerCertState {
    held(n.checkpoint())
}

fn cmd(client: u32, sn: u64, op: u64) -> ClientCommand {
    ClientCommand { client, sn, op }
}

fn timed(entry: Entry, round: u64) -> Timed {
    Timed { entry, round }
}

fn seed_log() -> Vec<Timed> {
    vec![timed(Entry::Nop(0), 0)]
}

fn genesis() -> Checkpoint {
    Checkpoint {
        s: SharedState::default().into(),
        p: None,
        w: 0,
        certs: Some(ServerCertState::new()),
    }
}

/// A healthy peer's reply: its log, checkpoint, and reset state.
fn reply(log: Option<Vec<Timed>>, checkpoint: Checkpoint, r: RState) -> RecoveryReply {
    RecoveryReply {
        l_j: log.map(Log::from_entries),
        c_j: checkpoint.into(),
        r_j: r,
    }
}

fn healthy_reply(log: &[Timed]) -> RecoveryReply {
    reply(Some(log.to_vec()), genesis(), RState::NoReset)
}

fn state_of(cmds: &[ClientCommand]) -> SharedState {
    SharedState::from_entries(
        cmds.iter().map(|c| Entry::Cmd(*c)).collect(),
        cmds.iter().map(|c| (c.client, c.sn)).collect(),
    )
}

fn cp(state: SharedState, pre: Option<Vec<Timed>>, window: u64) -> Checkpoint {
    Checkpoint {
        s: state.into(),
        p: pre,
        w: window,
        certs: Some(ServerCertState::new()),
    }
}

#[test]
fn wants_amplify_suppresses_present_commands_only() {
    let existing = cmd(1, 1, 1);
    let fresh = cmd(2, 1, 2);
    let mut n = node();
    let replies = vec![healthy_reply(&seed_log()); 3];
    n.step_chosen(&replies, &[(existing, 1)], Some(&[0, 1, 2]));

    assert!(!n.wants_amplify(&existing));
    assert!(n.wants_amplify(&fresh));
}

// --- the reply bullet: guarded by R_i ≠ ⊥ (not by L_i), carrying L_i only when it is non-⊥ ---

#[test]
fn answer_carries_log_checkpoint_and_r_exactly_while_r_is_not_bot() {
    let n = node();
    let reply = n.answer().expect("R = no-reset answers");
    assert_eq!(reply.r_j, RState::NoReset);
    assert_eq!(reply.l_j.map(|l| l.entries().to_vec()), n.log_entries());
    assert!(Arc::ptr_eq(&reply.c_j, &n.checkpoint_shared()));
}

#[test]
fn answer_is_guarded_by_r_not_by_the_log() {
    // R = reset with L = ⊥ (2 of 3 replies carry a log): the node still answers, without a log.
    let mut reset_without_log = node();
    reset_without_log.step(
        &[
            reply(Some(seed_log()), genesis(), RState::Reset),
            reply(Some(seed_log()), genesis(), RState::Reset),
            reply(None, genesis(), RState::Reset),
        ],
        &[],
        1,
        &mut rng(),
    );
    assert_eq!(reset_without_log.log_entries(), None, "precondition: L = ⊥");
    let reply = reset_without_log.answer().expect("R = reset answers");
    assert_eq!(reply.r_j, RState::Reset);
    assert!(reply.l_j.is_none());

    // R = ⊥ (no replies): the node does not answer.
    let mut bot = node();
    bot.step(&[], &[], 1, &mut rng());
    assert_eq!(bot.reset_state(), RState::Bot, "precondition: R = ⊥");
    assert!(bot.answer().is_none());
}

// --- during-window step 3: split ℓ-counts (Lemma 6.2, log-repliers ⊆ recovery-repliers) ---

#[test]
fn split_counts_update_r_but_bot_the_log() {
    // 3 recovery replies (≥ ℓ) of which only 2 carry a log (< ℓ): the
    // recovery layer runs, the log layer ⊥s.
    let mut n = node();
    let replies = vec![
        reply(Some(seed_log()), genesis(), RState::Reset),
        reply(Some(seed_log()), genesis(), RState::Reset),
        reply(None, genesis(), RState::Reset),
    ];
    n.step(&replies, &[], 1, &mut rng());
    assert_eq!(n.reset_state(), RState::Reset, "R updated over all 3");
    assert_eq!(n.log_entries(), None, "< ℓ log-carriers ⊥ the log");
}

// --- during-window step 3: the R-update scans ALL replies (Alg 6 box, p. 31) ---

#[test]
fn any_no_reset_reply_wins_over_all_received_replies_not_a_subsample() {
    // The box reads "if no-reset was received in one reply" over the whole
    // reply set; Definition 3.5's ℓ-subsample would (for this seed) draw
    // indices {0, 2, 1} of the k = 6 replies and miss the lone no-reset at
    // index 4.
    let mut n = node();
    let mut replies = vec![reply(None, genesis(), RState::Reset); 6];
    replies[4].r_j = RState::NoReset;
    n.step(&replies, &[], 1, &mut rng());
    assert_eq!(n.reset_state(), RState::NoReset);

    let mut n = node();
    let all_reset = vec![reply(None, genesis(), RState::Reset); 6];
    n.step(&all_reset, &[], 1, &mut rng());
    assert_eq!(n.reset_state(), RState::Reset);
}

// --- checkpoint adoption: largest W', adopt iff W' > W (Alg 6, p. 30–31) ---

#[test]
fn newer_window_checkpoint_adoption_overwrites_state_log_and_checkpoint() {
    let mut n = node();
    let a = cmd(1, 1, 77);
    let b = cmd(2, 1, 88);
    let pre2 = vec![timed(Entry::Cmd(a), 3)];
    let cp2 = cp(state_of(&[a]), Some(pre2.clone()), 2);
    n.step(
        &vec![reply(Some(seed_log()), cp2.clone(), RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    assert_eq!(*n.shared_state(), state_of(&[a]));
    assert_eq!(n.log_entries(), Some(pre2.clone()));
    assert_eq!(n.checkpoint().w, 2);

    // Newer W' = 4 overwrites again, original round stamps preserved.
    let pre4 = vec![timed(Entry::Cmd(a), 3), timed(Entry::Cmd(b), 7)];
    let cp4 = cp(state_of(&[a, b]), Some(pre4.clone()), 4);
    n.step(
        &vec![reply(Some(pre2.clone()), cp4.clone(), RState::NoReset); 3],
        &[],
        8,
        &mut rng(),
    );
    assert_eq!(*n.shared_state(), state_of(&[a, b]));
    assert_eq!(n.log_entries(), Some(pre4.clone()));
    assert_eq!(n.log_entries().unwrap()[0].round, 3, "stamps preserved");
    assert_eq!(*n.checkpoint(), cp4);

    // Equal (W' = 4) and older (W' = 1) checkpoints leave everything alone;
    // replies echo the node's own log so the median layer is a fixpoint.
    let own_log = n.log_entries().unwrap().to_vec();
    let cp1 = cp(state_of(&[b]), Some(vec![timed(Entry::Cmd(b), 1)]), 1);
    n.step(
        &[
            reply(Some(own_log.clone()), cp4.clone(), RState::NoReset),
            reply(Some(own_log.clone()), cp1, RState::NoReset),
            reply(Some(own_log.clone()), cp4.clone(), RState::NoReset),
        ],
        &[],
        9,
        &mut rng(),
    );
    assert_eq!(*n.shared_state(), state_of(&[a, b]));
    assert_eq!(n.log_entries(), Some(pre4.clone()));
    assert_eq!(*n.checkpoint(), cp4);
}

#[test]
fn tied_largest_window_checkpoints_may_resolve_either_way() {
    let mut n = node();
    let a = cmd(1, 1, 77);
    let b = cmd(2, 1, 88);
    let cp_a = cp(state_of(&[a]), Some(vec![timed(Entry::Cmd(a), 2)]), 4);
    let cp_b = cp(state_of(&[b]), Some(vec![timed(Entry::Cmd(b), 2)]), 4);
    n.step(
        &[
            reply(None, cp_a.clone(), RState::NoReset),
            reply(None, cp_b.clone(), RState::NoReset),
            reply(None, cp_a.clone(), RState::NoReset),
        ],
        &[],
        3,
        &mut rng(),
    );
    let adopted = n.checkpoint();
    assert!(
        *adopted == cp_a || *adopted == cp_b,
        "ties break arbitrarily but must adopt one of the tied checkpoints"
    );
    assert_eq!(*n.shared_state(), *adopted.s);
    assert_eq!(n.log_entries(), adopted.p.clone());
}

/// A checkpoint of window `w` holding the one command `client`, so every window is distinguishable.
fn window_checkpoint(w: u64) -> Checkpoint {
    let c = cmd(w as u32, 1, 100 + w);
    cp(state_of(&[c]), Some(vec![timed(Entry::Cmd(c), 2)]), w)
}

#[test]
fn the_largest_window_among_mixed_replies_is_adopted_wherever_it_sits() {
    // Alg 6: "the received checkpoint with largest W'" over ALL replies, not the first, the
    // last or the smallest. Windows 1, 3, 2 in every order, then six replies with the
    // maximum at each position.
    let orders: [[u64; 3]; 6] = [
        [1, 3, 2],
        [1, 2, 3],
        [3, 1, 2],
        [3, 2, 1],
        [2, 1, 3],
        [2, 3, 1],
    ];
    for order in orders {
        let mut n = node();
        let replies: Vec<_> = order
            .iter()
            .map(|&w| reply(Some(seed_log()), window_checkpoint(w), RState::NoReset))
            .collect();
        n.step(&replies, &[], 4, &mut rng());
        assert_eq!(n.checkpoint().w, 3, "windows {order:?}");
        assert_eq!(*n.checkpoint(), window_checkpoint(3), "windows {order:?}");
    }
    for position in 0..6 {
        let mut n = node();
        let replies: Vec<_> = (0..6)
            .map(|i| {
                let w = if i == position { 9 } else { 1 + i as u64 % 4 };
                reply(Some(seed_log()), window_checkpoint(w), RState::NoReset)
            })
            .collect();
        n.step(&replies, &[], 4, &mut rng());
        assert_eq!(n.checkpoint().w, 9, "maximum at reply {position}");
    }
}

#[test]
fn an_equal_window_checkpoint_with_other_contents_is_not_adopted() {
    // Alg 6: "If W' > W": W' = W adopts nothing, even when S', P' differ from the node's own.
    let mut n = node();
    let own = window_checkpoint(2);
    n.step(
        &vec![reply(Some(seed_log()), own.clone(), RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    assert_eq!(*n.checkpoint(), own);
    let own_log = n.log_entries().unwrap().to_vec();
    let other = cp(
        state_of(&[cmd(7, 1, 700)]),
        Some(vec![timed(Entry::Cmd(cmd(7, 1, 700)), 2)]),
        2,
    );
    n.step(
        &vec![reply(Some(own_log.clone()), other, RState::NoReset); 3],
        &[],
        5,
        &mut rng(),
    );
    assert_eq!(*n.checkpoint(), own, "equal W' leaves C_i alone");
    assert_eq!(*n.shared_state(), *own.s);
    assert_eq!(n.log_entries(), Some(own_log));
}

#[test]
fn a_newer_checkpoint_carried_by_fewer_than_ell_replies_is_not_adopted() {
    // Alg 6: with less than ℓ replies the node sets R_i := ⊥ and adopts nothing, however new
    // the checkpoints in those replies are.
    let mut n = node();
    let replies = vec![reply(Some(seed_log()), window_checkpoint(5), RState::NoReset); 2];
    n.step(&replies, &[], 4, &mut rng());
    assert_eq!(n.reset_state(), RState::Bot);
    assert_eq!(*n.checkpoint(), genesis());
    assert_eq!(*n.shared_state(), SharedState::default());
}

#[test]
fn a_tie_on_the_largest_window_resolves_to_the_last_tied_reply() {
    let a = cmd(1, 1, 77);
    let b = cmd(2, 1, 88);
    let cp_a = cp(state_of(&[a]), Some(vec![timed(Entry::Cmd(a), 2)]), 4);
    let cp_b = cp(state_of(&[b]), Some(vec![timed(Entry::Cmd(b), 2)]), 4);
    let older = cp(state_of(&[]), Some(Vec::new()), 2);
    for (replies, last_tied, expected) in [
        (
            vec![
                reply(None, cp_a.clone(), RState::NoReset),
                reply(None, cp_b.clone(), RState::NoReset),
                reply(None, older.clone(), RState::NoReset),
            ],
            1,
            &cp_b,
        ),
        (
            vec![
                reply(None, cp_b.clone(), RState::NoReset),
                reply(None, cp_a.clone(), RState::NoReset),
                reply(None, cp_a.clone(), RState::NoReset),
            ],
            2,
            &cp_a,
        ),
    ] {
        assert_eq!(latest_checkpoint_index(&replies), last_tied);
        let mut n = node();
        n.step(&replies, &[], 3, &mut rng());
        assert_eq!(n.checkpoint(), expected);
        assert!(Arc::ptr_eq(&n.checkpoint_shared(), &replies[last_tied].c_j));
    }
}

#[test]
fn adoption_overrides_same_round_median_and_appends() {
    // Intra-round order is (1) log median, (2) R-update, (3) adoption LAST:
    // the adopting server was checkpoint-stale, so the median result and the
    // same-round append are deliberately lost ("immediately adopts", p. 30).
    let mut n = node();
    let a = cmd(1, 1, 77);
    let x = cmd(2, 1, 88);
    let y = cmd(3, 1, 99);
    let pre4 = vec![timed(Entry::Cmd(a), 2)];
    let cp4 = cp(state_of(&[a]), Some(pre4.clone()), 4);
    let mut peer_log = seed_log();
    peer_log.push(timed(Entry::Cmd(x), 3));
    n.step(
        &vec![reply(Some(peer_log), cp4, RState::NoReset); 3],
        &[(y, 5)],
        5,
        &mut rng(),
    );
    assert_eq!(
        n.log_entries(),
        Some(pre4.clone()),
        "final log is P' exactly"
    );
    assert!(
        !n.log_entries()
            .unwrap()
            .iter()
            .any(|t| t.entry == Entry::Cmd(y)),
        "the same-round append is discarded by adoption"
    );
}

#[test]
fn adopted_empty_pre_leaves_empty_non_bot_log_without_mid_window_dummy() {
    // A drained-but-healthy server's checkpoint carries P = Some([]) — not ⊥.
    // Adoption leaves an empty non-⊥ log; x_d is boundary-only in the box.
    let mut n = node();
    let cp1 = cp(state_of(&[cmd(1, 1, 77)]), Some(Vec::new()), 1);
    n.step(
        &vec![reply(None, cp1, RState::NoReset); 3],
        &[],
        2,
        &mut rng(),
    );
    assert_eq!(
        n.log_entries(),
        Some(Vec::new()),
        "empty but non-⊥, no dummy"
    );
}

// --- between-window step 2: minting (Alg 6, p. 31) ---

#[test]
fn checkpoint_pre_is_the_longest_aged_prefix_and_a_literal_prefix_of_the_log() {
    // Ages at round 12 (T = 10): [10, 10, 7, 11] — the young third entry
    // blocks the aged fourth from P ("longest prefix ... with age ≥ T").
    let mut n = node();
    let log4 = vec![
        timed(Entry::Cmd(cmd(1, 1, 77)), 2),
        timed(Entry::Cmd(cmd(2, 1, 88)), 2),
        timed(Entry::Cmd(cmd(3, 1, 99)), 5),
        timed(Entry::Cmd(cmd(4, 1, 66)), 1),
    ];
    n.step(&vec![healthy_reply(&log4); 3], &[], 6, &mut rng());
    assert_eq!(n.log_entries(), Some(log4.clone()));
    n.end_window(1, 12);
    assert_eq!(n.checkpoint().p, Some(log4[..2].to_vec()));
    assert_eq!(n.checkpoint().w, 1);
    assert_eq!(
        n.log_entries(),
        Some(log4.clone()),
        "genesis P is ⊥: nothing commits, the full log stays"
    );
    assert!(
        n.shared_state().untruncated().is_empty(),
        "P_new is pre-committed, not executed"
    );
}

#[test]
fn first_end_window_mints_window_one_and_rollback_cascades_through_recommit() {
    // First T-window is number 0; the first boundary mints W = 1 with
    // P = [x₀] carrying its ORIGINAL round stamp 0 (aged exactly T).
    let mut n = node();
    n.end_window(1, T);
    assert_eq!(n.checkpoint().w, 1);
    assert_eq!(n.checkpoint().p, Some(seed_log()));

    // Force Reset via an all-reset round (replies echo the node's own log so
    // the median layer is a fixpoint; genesis W' = 0 adopts nothing).
    let own = n.log_entries().unwrap().to_vec();
    n.step(
        &vec![reply(Some(own), genesis(), RState::Reset); 3],
        &[],
        T + 1,
        &mut rng(),
    );
    assert_eq!(n.reset_state(), RState::Reset);

    // Rollback restores L := P un-restamped and the SAME boundary re-commits
    // it (sequential between-window steps): the node exits in NoReset at
    // W = 2 with a fresh dummy — never in Reset with L = P.
    n.end_window(2, 2 * T);
    assert_eq!(n.reset_state(), RState::NoReset);
    assert_eq!(n.checkpoint().w, 2);
    assert_eq!(
        n.log_entries(),
        Some(vec![timed(Entry::Nop(2 * T), 2 * T)]),
        "P committed and stripped, dummy appended"
    );
    assert!(
        n.shared_state().untruncated().is_empty(),
        "x₀ commits as a no-op"
    );
}

// --- between-window steps are sequential, not exclusive branches (p. 31) ---

#[test]
fn rollback_then_recommit_cascade_completes_in_one_boundary() {
    // The KB-flagged trap: a server entering the boundary in Reset with
    // P ≠ ⊥ rolls back (step 1) AND falls through into step 2 in the SAME
    // call — commit P, strip, fresh checkpoint, exit NoReset. An if/else-if
    // reading would exit in Reset with log = P, adding a full window to
    // every recovery and breaking Lemma 6.10's 3T bound.
    let a = cmd(1, 1, 77);
    let b = cmd(2, 1, 88);
    let c = cmd(3, 1, 99);
    let pre3 = vec![timed(Entry::Cmd(b), 2), timed(Entry::Cmd(c), 2)];
    let cp3 = cp(state_of(&[a]), Some(pre3.clone()), 3);

    let mut n = node();
    n.step(
        &vec![reply(None, cp3.clone(), RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    assert_eq!(n.log_entries(), Some(pre3.clone()), "adopted W = 3");

    let own = n.log_entries().unwrap().to_vec();
    n.step(
        &vec![reply(Some(own), cp3, RState::Reset); 3],
        &[],
        5,
        &mut rng(),
    );
    assert_eq!(n.reset_state(), RState::Reset);

    n.end_window(4, 40);
    assert_eq!(
        n.shared_state().untruncated(),
        vec![Entry::Cmd(a), Entry::Cmd(b), Entry::Cmd(c)],
        "P's commands executed onto S's in one boundary"
    );
    assert_eq!(
        n.log_entries(),
        Some(vec![timed(Entry::Nop(40), 40)]),
        "P stripped; the node does NOT exit in Reset with log = P"
    );
    assert_eq!(n.checkpoint().w, 4);
    assert_eq!(n.reset_state(), RState::NoReset);
}

// --- the P ⊑ L strip precondition must fail loudly in every build profile ---

/// A node whose checkpoint P the log no longer extends — reachable below the
/// T floor via same-W checkpoint forks (the cliff regime): adopt (S', [a], 1),
/// then let the median replace the log wholesale from peers on the other
/// lineage.
fn forked_node(peer_log: Vec<Timed>) -> RecoveryNode {
    let a = cmd(1, 1, 77);
    let cp1 = cp(state_of(&[a]), Some(vec![timed(Entry::Cmd(a), 2)]), 1);
    let mut n = node();
    n.step(
        &vec![reply(None, cp1, RState::NoReset); 3],
        &[],
        3,
        &mut rng(),
    );
    n.step(
        &vec![reply(Some(peer_log), genesis(), RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    n
}

fn assert_valid_boundary_is_read_only(n: &RecoveryNode) {
    let checkpoint = n.checkpoint_shared();
    let state = std::sync::Arc::clone(n.state_arc());
    let log = n.log_perm_arc().cloned();
    let r = n.reset_state();
    assert!(n.validate_boundary().is_ok());
    assert!(std::sync::Arc::ptr_eq(&checkpoint, &n.checkpoint_shared()));
    assert!(std::sync::Arc::ptr_eq(&state, n.state_arc()));
    assert_eq!(r, n.reset_state());
    match (log, n.log_perm_arc()) {
        (None, None) => {}
        (Some(before), Some(after)) => {
            assert!(std::sync::Arc::ptr_eq(before.entries(), after.entries()));
            assert!(std::sync::Arc::ptr_eq(before.perm(), after.perm()));
        }
        _ => panic!("validation changed log bottomness"),
    }
}

#[test]
fn boundary_validation_reports_bounded_stamped_diagnostics_without_mutating() {
    let expected = timed(Entry::Cmd(cmd(1, 1, 77)), 2);
    let actual = timed(Entry::Cmd(cmd(2, 1, 88)), 2);
    let n = forked_node(vec![actual.clone()]);
    let before_log = n.log_entries();
    let before_state = n.shared_state().clone();
    let before_checkpoint = n.checkpoint().clone();

    let violation = n.validate_boundary().expect_err("forked prefix");
    assert_eq!(violation.pre_len, 1);
    assert_eq!(violation.log_len, 1);
    assert_eq!(violation.first_mismatch, 0);
    assert_eq!(violation.expected, Some(expected));
    assert_eq!(violation.actual, Some(actual));
    assert_eq!(n.log_entries(), before_log);
    assert_eq!(*n.shared_state(), before_state);
    assert_eq!(*n.checkpoint(), before_checkpoint);
}

#[test]
fn boundary_validation_accepts_a_stamp_only_difference_and_rejects_a_short_log() {
    let a = cmd(1, 1, 77);
    let stamp_only = forked_node(vec![timed(Entry::Cmd(a), 9)]);
    assert_valid_boundary_is_read_only(&stamp_only);

    let short = forked_node(Vec::new());
    let short = short.validate_boundary().expect_err("short log");
    assert_eq!(short.first_mismatch, 0);
    assert_eq!(short.expected, Some(timed(Entry::Cmd(a), 2)));
    assert_eq!(short.actual, None);
}

#[test]
fn boundary_validation_accepts_genesis_bottom_and_empty_prefix() {
    let genesis = node();
    assert_valid_boundary_is_read_only(&genesis);

    let mut bottom = node();
    bottom.step(&[], &[], 1, &mut rng());
    assert_eq!(bottom.log_entries(), None);
    assert_valid_boundary_is_read_only(&bottom);

    let mut empty = node();
    let checkpoint = cp(SharedState::default(), Some(Vec::new()), 1);
    empty.step(
        &vec![reply(None, checkpoint, RState::NoReset); 3],
        &[],
        2,
        &mut rng(),
    );
    assert_eq!(empty.checkpoint().p, Some(Vec::new()));
    assert_valid_boundary_is_read_only(&empty);
}

#[test]
fn reset_validation_uses_checkpoint_restored_log_not_stale_current_log() {
    let mut n = forked_node(vec![timed(Entry::Cmd(cmd(2, 1, 88)), 2)]);
    let replies = vec![
        reply(n.log_entries(), n.checkpoint().clone(), RState::Reset),
        reply(n.log_entries(), n.checkpoint().clone(), RState::Reset),
        reply(n.log_entries(), n.checkpoint().clone(), RState::Reset),
    ];
    n.step(&replies, &[], 5, &mut rng());
    assert_eq!(n.reset_state(), RState::Reset);
    assert_valid_boundary_is_read_only(&n);
}

#[test]
#[should_panic(expected = "P must be a prefix of L")]
fn boundary_strip_on_a_non_extending_log_panics_labeled() {
    // Same length, different content: an unguarded strip-by-length would
    // silently remove the wrong entry while committing P's.
    let mut n = forked_node(vec![timed(Entry::Cmd(cmd(2, 1, 88)), 2)]);
    n.end_window(2, 20);
}

#[test]
#[should_panic(expected = "P must be a prefix of L")]
fn boundary_strip_on_a_log_shorter_than_p_panics_labeled() {
    let mut n = forked_node(Vec::new());
    n.end_window(2, 20);
}

// --- opt-in repair: skip a boundary whose P is not a prefix of L_i ---

fn skipped_node() -> RecoveryNode {
    let mut n = forked_node(vec![timed(Entry::Cmd(cmd(2, 1, 88)), 2)])
        .with_prefix_mismatch(PrefixMismatch::SkipBoundary);
    n.end_window(2, 20);
    n
}

#[test]
fn skip_boundary_commits_nothing_drops_the_log_then_adopts_the_next_window() {
    let b = cmd(2, 1, 88);
    let peer_log = vec![timed(Entry::Cmd(b), 2)];
    let mut n = forked_node(peer_log.clone()).with_prefix_mismatch(PrefixMismatch::SkipBoundary);
    let checkpoint = n.checkpoint_shared();
    let state = n.shared_state().clone();

    n.end_window(2, 20);
    assert!(
        Arc::ptr_eq(&checkpoint, &n.checkpoint_shared()),
        "no new checkpoint"
    );
    assert_eq!(n.checkpoint().w, 1, "W stays at the old window");
    assert_eq!(*n.shared_state(), state, "P is not executed");
    // Peers strip what they commit here; an unstripped L_i would gossip it back for a second commit.
    assert_eq!(n.log_entries(), None, "the unstripped log is dropped");
    assert_eq!(n.reset_state(), RState::Reset, "Alg 6: L_i = ⊥ sets reset");
    assert!(n.stale_checkpoint(), "the skip marks C_i stale");

    let cp2 = cp(state_of(&[b]), Some(Vec::new()), 2);
    n.step(
        &vec![reply(Some(peer_log), cp2.clone(), RState::NoReset); 3],
        &[],
        21,
        &mut rng(),
    );
    assert_eq!(*n.checkpoint(), cp2, "W' = 2 > 1 is adopted");
    assert_eq!(*n.shared_state(), state_of(&[b]));
    assert_eq!(n.log_entries(), Some(Vec::new()));
    assert_eq!(n.reset_state(), RState::NoReset);
    assert!(!n.stale_checkpoint(), "adoption clears the mark");

    n.end_window(3, 30);
    assert_eq!(
        n.checkpoint().w,
        3,
        "the adopted node takes the next boundary"
    );
}

#[test]
fn skip_boundary_never_commits_the_skipped_p_when_the_log_matches_again() {
    let a = cmd(1, 1, 77);
    let mut n = skipped_node();
    let checkpoint = n.checkpoint_shared();
    let state = n.shared_state().clone();
    n.step(
        &vec![
            reply(
                Some(vec![timed(Entry::Cmd(a), 2)]),
                genesis(),
                RState::NoReset
            );
            3
        ],
        &[],
        21,
        &mut rng(),
    );
    assert!(
        n.validate_boundary().is_ok(),
        "precondition: P prefixes L_i again"
    );
    assert_eq!(n.would_carry_pre(30), None, "a stale node mints nothing");

    n.end_window(3, 30);
    assert!(Arc::ptr_eq(&checkpoint, &n.checkpoint_shared()));
    assert_eq!(*n.shared_state(), state, "the skipped P never commits");
    assert_eq!(n.log_entries(), None);
}

#[test]
fn skip_boundary_stale_node_in_reset_does_not_restore_the_skipped_p() {
    let mut n = skipped_node();
    let checkpoint = n.checkpoint_shared();
    let state = n.shared_state().clone();
    let peer_log = Some(vec![timed(Entry::Cmd(cmd(3, 1, 99)), 21)]);
    n.step(
        &vec![reply(peer_log, genesis(), RState::Reset); 3],
        &[],
        22,
        &mut rng(),
    );
    assert_eq!(n.reset_state(), RState::Reset, "precondition: a reset vote");
    assert!(n.log_entries().is_some(), "precondition: a merged log");

    n.end_window(3, 30);
    assert!(Arc::ptr_eq(&checkpoint, &n.checkpoint_shared()));
    assert_eq!(*n.shared_state(), state, "the skipped P never commits");
    assert_eq!(
        n.log_entries(),
        None,
        "L_i is not rolled back to the skipped P"
    );
}

// "Never commits" holds for the skipping node only: it still answers with its stale C_i (R_i = reset
// is not ⊥), and a peer with an older W adopts it as an ordinary newer checkpoint.
#[test]
fn a_stale_checkpoint_is_still_served_and_an_older_peer_adopts_and_commits_it() {
    let skipper = skipped_node();
    assert!(skipper.stale_checkpoint(), "precondition: C_i is stale");
    let served = skipper.answer().expect("R_i = reset answers");
    assert!(Arc::ptr_eq(&served.c_j, &skipper.checkpoint_shared()));

    let mut peer = node();
    peer.step(&vec![served; 3], &[], 21, &mut rng());
    assert_eq!(
        peer.checkpoint().w,
        1,
        "the older peer adopts the stale C_i"
    );
    assert!(!peer.stale_checkpoint(), "adoption clears the mark");

    peer.end_window(2, 30);
    assert_eq!(peer.checkpoint().w, 2, "the adopter mints the next window");
    assert_eq!(
        peer.shared_state().logical_len(),
        skipper.shared_state().logical_len() + 1,
        "and commits the P the skipper never commits"
    );
}

#[test]
fn rollback_to_genesis_checkpoint_stays_reset() {
    // Genesis P = ⊥, not empty: step 1 leaves L = ⊥, step 2 is skipped, step
    // 3 re-arms Reset. With P wrongly modeled as an empty vec, step 2 would
    // fire and mint a fresh W = 1 checkpoint carrying s₀-era content — the
    // fork Lemma 6.5 excludes.
    let mut n = node();
    let own = n.log_entries().unwrap().to_vec();
    n.step(
        &vec![reply(Some(own), genesis(), RState::Reset); 3],
        &[],
        1,
        &mut rng(),
    );
    assert_eq!(n.reset_state(), RState::Reset);

    n.end_window(1, T);
    assert_eq!(n.log_entries(), None, "genesis rollback lands in ⊥, not []");
    assert_eq!(n.reset_state(), RState::Reset, "step 3 re-arms Reset");
    assert_eq!(*n.checkpoint(), genesis(), "no checkpoint minted");
    assert_eq!(*n.shared_state(), SharedState::default());
}

#[test]
fn drained_healthy_log_gets_dummy_and_stays_no_reset_at_next_boundary() {
    // x_d exists so "drained but healthy" stays distinguishable from
    // "blocked": commit-and-strip empties the log, the dummy keeps it
    // non-empty, and the node never self-diagnoses as blocked (p. 30).
    let a = cmd(1, 1, 77);
    let mut n = node();
    n.step(
        &vec![healthy_reply(&[timed(Entry::Cmd(a), 2)]); 3],
        &[],
        3,
        &mut rng(),
    );
    n.end_window(1, 12);
    assert_eq!(
        n.log_entries(),
        n.checkpoint().p.clone(),
        "precondition: nothing uncommitted — log is exactly P"
    );

    n.end_window(2, 20);
    assert_eq!(n.log_entries(), Some(vec![timed(Entry::Nop(20), 20)]));
    assert_eq!(n.shared_state().untruncated(), vec![Entry::Cmd(a)]);
    assert_eq!(n.reset_state(), RState::NoReset);

    n.end_window(3, 30);
    assert_eq!(
        n.reset_state(),
        RState::NoReset,
        "idle window stays NoReset"
    );
}

#[test]
fn end_window_mints_the_passed_next_window_not_own_plus_one() {
    // After a multi-window ⊥ gap the node's own W is stale; the mint carries
    // the driver's global window index ("W' is the number of the next
    // T-window", p. 31) — a self-incrementing counter would desync W and
    // corrupt age priority after long surges.
    let mut n = node();
    let a = cmd(1, 1, 77);
    let cp2 = cp(state_of(&[a]), Some(vec![timed(Entry::Cmd(a), 3)]), 2);
    n.step(
        &vec![reply(None, cp2, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    assert_eq!(n.checkpoint().w, 2, "precondition: stale W = 2");
    n.end_window(7, 70);
    assert_eq!(n.checkpoint().w, 7);
}

#[test]
fn adopted_entry_keeps_the_reply_round_stamp() {
    // Stamps travel with entries, so age is effectively global (compact
    // precedent): a re-stamping merge would let a boundary-straddling
    // command enter one server's P but not another's, breaking Lemma 6.7's
    // same-checkpoint identity.
    let mut n = node();
    let a = cmd(1, 1, 77);
    let mut peer_log = seed_log();
    peer_log.push(timed(Entry::Cmd(a), 3));
    n.step(&vec![healthy_reply(&peer_log); 3], &[], 9, &mut rng());
    let live = n.log_entries().unwrap();
    let got = live
        .iter()
        .find(|t| t.entry == Entry::Cmd(a))
        .expect("entry adopted from the replies");
    assert_eq!(got.round, 3, "sender's stamp, not the receiving round 9");
}

// --- monotonicity smoke (Lemma 6.9): rollback rewinds the log, never executed ---

#[test]
fn committed_entries_never_leave_executed_across_rollback() {
    let a = cmd(1, 1, 77);
    let b = cmd(2, 1, 88);
    let mut n = node();
    n.step(
        &vec![healthy_reply(&[timed(Entry::Cmd(a), 2)]); 3],
        &[],
        3,
        &mut rng(),
    );
    n.end_window(1, 12);
    n.end_window(2, 20);
    assert_eq!(n.shared_state().untruncated(), vec![Entry::Cmd(a)]);

    // Uncommitted b lands mid-window, then the node is forced into Reset.
    let own_cp = n.checkpoint().clone();
    let own = n.log_entries().unwrap().to_vec();
    n.step(
        &vec![reply(Some(own), own_cp.clone(), RState::NoReset); 3],
        &[(b, 25)],
        25,
        &mut rng(),
    );
    assert!(
        n.log_entries()
            .unwrap()
            .iter()
            .any(|t| t.entry == Entry::Cmd(b)),
        "precondition: b is in the log, uncommitted"
    );
    let own = n.log_entries().unwrap().to_vec();
    n.step(
        &vec![reply(Some(own), own_cp, RState::Reset); 3],
        &[],
        26,
        &mut rng(),
    );
    assert_eq!(n.reset_state(), RState::Reset);

    let before = n.shared_state().untruncated().to_vec();
    n.end_window(3, 30);
    let after = n.shared_state().untruncated();
    assert!(
        after.iter().take(before.len()).eq(before.iter()),
        "executed before the rollback boundary is a prefix of executed after"
    );
    assert!(
        !n.log_entries()
            .unwrap()
            .iter()
            .any(|t| t.entry == Entry::Cmd(b)),
        "the rollback rewinds the LOG: uncommitted b is discarded"
    );
}

// --- invariant hammers (Lemma 6.2 one-way; derived state ≡ checkpoint state) ---

/// Drives one node through 200 random operations — reply sets above/below ℓ,
/// mixed R values, mixed log/no-log, checkpoints with varying W, interleaved
/// boundaries — calling `check` after every operation. Reply logs extend the
/// prefix that will bind at adoption time, as any real population's do (P is
/// a literal prefix of every log descended from its checkpoint). Returns
/// whether the `Reset ∧ L = ⊥` limbo state was visited.
///
/// A TWIN is driven through the identical operations and takes the shared mint
/// wherever the primary minted at all, so every invariant hammered here is
/// hammered on `end_window_shared` too — the twin's inputs are equal by
/// construction, which is exactly the premise a driver's class key has to
/// prove.
fn hammer(seed: u64, mut check: impl FnMut(&RecoveryNode)) -> bool {
    use rand::Rng;
    let mut g = ChaCha12Rng::seed_from_u64(seed);
    let mut n = node();
    let mut twin = node();
    let mut wmax: u64 = 0;
    let mut saw_limbo = false;
    let mut round: u64 = 0;
    for _ in 0..200 {
        round += g.random_range(1..4);
        if g.random_range(0..100) < 80 {
            let count = g.random_range(0..=8);
            let cps: Vec<Checkpoint> = (0..count)
                .map(|_| {
                    let w = g.random_range(0..=wmax);
                    if w == 0 {
                        genesis()
                    } else {
                        let body = cmd(g.random_range(1..5), 1, g.random_range(10..99));
                        let pre: Vec<Timed> = (0..g.random_range(0..3))
                            .map(|_| {
                                timed(
                                    Entry::Cmd(cmd(
                                        g.random_range(1..9),
                                        1,
                                        g.random_range(100..999),
                                    )),
                                    g.random_range(0..=round),
                                )
                            })
                            .collect();
                        cp(state_of(&[body]), Some(pre), w)
                    }
                })
                .collect();
            let best = cps.iter().max_by_key(|c| c.w);
            let binding: Vec<Timed> = match best {
                Some(b) if b.w > n.checkpoint().w => b.p.clone().unwrap_or_default(),
                _ => n.checkpoint().p.clone().unwrap_or_default(),
            };
            let replies: Vec<RecoveryReply> = cps
                .into_iter()
                .map(|checkpoint| {
                    let log = g.random_bool(0.5).then(|| {
                        let mut l = binding.clone();
                        for _ in 0..g.random_range(0..3) {
                            l.push(timed(
                                Entry::Cmd(cmd(
                                    g.random_range(1..9),
                                    1,
                                    g.random_range(1000..9999),
                                )),
                                g.random_range(0..=round),
                            ));
                        }
                        l
                    });
                    let r = if g.random_bool(0.5) {
                        RState::NoReset
                    } else {
                        RState::Reset
                    };
                    RecoveryReply {
                        l_j: log.map(Log::from_entries),
                        c_j: checkpoint.into(),
                        r_j: r,
                    }
                })
                .collect();
            let appends: Vec<(ClientCommand, u64)> = if g.random_bool(0.3) {
                {
                    vec![(
                        cmd(g.random_range(1..9), 1, g.random_range(10000..99999)),
                        round,
                    )]
                }
            } else {
                Default::default()
            };
            // The twin steps from the same stream position, so the two take
            // the same draw and stay in lockstep.
            let mut twin_g = g.clone();
            n.step(&replies, &appends, round, &mut g);
            twin.step(&replies, &appends, round, &mut twin_g);
        } else {
            wmax += 1;
            n.end_window(wmax, round);
            if n.checkpoint().w == wmax {
                twin.end_window_shared(wmax, round, &n.checkpoint_shared());
            } else {
                twin.end_window(wmax, round);
            }
        }
        assert_eq!(
            (
                n.checkpoint(),
                n.log_entries(),
                n.reset_state(),
                n.shared_state()
            ),
            (
                twin.checkpoint(),
                twin.log_entries(),
                twin.reset_state(),
                twin.shared_state()
            ),
            "seed {seed}: the shared mint left the twin somewhere the private mint did not"
        );
        check(&n);
        if n.reset_state() == RState::Reset && n.log_entries().is_none() {
            saw_limbo = true;
        }
    }
    saw_limbo
}

#[test]
fn bot_implies_bot_log_after_every_operation_under_random_reply_hammer() {
    let mut limbo_seen = false;
    for seed in 0..50 {
        limbo_seen |= hammer(seed, |n| {
            assert!(
                n.reset_state() != RState::Bot || n.log_entries().is_none(),
                "seed {seed}: Lemma 6.2 violated — R = ⊥ with a live log"
            );
        });
    }
    assert!(
        limbo_seen,
        "anti-vacuity: the legal Reset ∧ L = ⊥ limbo state must occur"
    );
}

#[test]
fn state_equals_checkpoint_state_after_every_public_operation() {
    // Derived invariant, verified against the box: S_i changes only at
    // adoption (which also sets C_i) and at boundary step 2 (which
    // immediately re-checkpoints), so Alg 6's rollback assignment S_i := S
    // is state-vacuous — per-server state can never regress (Lemma 6.9).
    for seed in 0..50 {
        hammer(seed, |n| {
            assert_eq!(
                *n.shared_state(),
                *n.checkpoint().s,
                "seed {seed}: S_i diverged from its checkpoint state"
            );
        });
    }
}

// --- §5 issuance at the boundary: the forest is over committed commands ---

/// The induction invariant a minted checkpoint must satisfy: its forest is
/// exactly the forest over its own committed sequence. Oracles may read
/// `executed`; the construction path may not.
fn assert_certs_match_executed(n: &RecoveryNode, label: &str) {
    let executed = n.checkpoint().s.untruncated();
    let mut twin = MmrForest::new();
    for e in executed.iter() {
        twin.append(leaf_hash(e));
    }
    assert_eq!(
        certs(n).forest().len(),
        executed.len() as u64,
        "{label}: forest length tracks the committed command count"
    );
    assert_eq!(
        certs(n).forest().roots(),
        twin.roots(),
        "{label}: roots equal the twin forest over the committed sequence"
    );
}

/// pp. 27–28's storage requirement is §5's, not Algorithm 6's: a server
/// running Algorithm 6 as printed stores no forest and no per-client window, and
/// boundary that would have attested P simply has nothing to attest. The
/// executed state is identical either way, which is what makes the layer a
/// configuration choice rather than a fidelity cut.
#[test]
fn base_algorithm_6_commits_the_same_state_and_stores_no_certificates() {
    let (a, b, c) = (cmd(1, 1, 11), cmd(2, 1, 22), cmd(3, 1, 33));
    let log = vec![
        timed(Entry::Nop(0), 0),
        timed(Entry::Cmd(a), 2),
        timed(Entry::Cmd(b), 2),
        timed(Entry::Cmd(c), 2),
    ];
    let mut bare = RecoveryNode::new(Config::default(), T, false);
    let mut mounted = node();
    for n in [&mut bare, &mut mounted] {
        n.step(&vec![healthy_reply(&log); 3], &[], 6, &mut rng());
        for w in 1..=3 {
            n.end_window(w, w * T);
        }
    }
    assert!(
        bare.checkpoint().certs.is_none(),
        "Alg 6 as printed stores no §5 state"
    );
    assert_eq!(certs(&mounted).forest().len(), 3, "anti-vacuity");
    assert_eq!(
        bare.checkpoint().s.untruncated(),
        mounted.checkpoint().s.untruncated(),
        "the layer changed what the server committed"
    );
    assert_eq!(
        bare.checkpoint().p,
        mounted.checkpoint().p,
        "the layer changed P"
    );
    assert_eq!(bare.checkpoint(), mounted.checkpoint());
}

#[test]
fn happy_boundaries_append_exactly_the_committed_commands_to_the_forest() {
    // Commitment is boundary-only in Alg 6: a P-entry is pre-committed and
    // revocable, so nothing may enter the forest before it executes.
    let (a, b, c) = (cmd(1, 1, 11), cmd(2, 1, 22), cmd(3, 1, 33));
    let mut n = node();
    let log = vec![
        timed(Entry::Nop(0), 0),
        timed(Entry::Cmd(a), 2),
        timed(Entry::Cmd(b), 2),
        timed(Entry::Cmd(c), 2),
    ];
    n.step(&vec![healthy_reply(&log); 3], &[], 6, &mut rng());

    n.end_window(1, T);
    assert_eq!(
        n.checkpoint().p,
        Some(log[..1].to_vec()),
        "only x₀ has aged; a, b, c stay young"
    );
    assert_eq!(
        certs(&n).forest().len(),
        0,
        "genesis P is ⊥ — nothing commits, nothing is issuable"
    );
    assert_certs_match_executed(&n, "boundary 1");

    n.end_window(2, 2 * T);
    assert_eq!(
        n.checkpoint().p,
        Some(log[1..].to_vec()),
        "a, b, c aged into P but are only PRE-committed"
    );
    assert_eq!(
        certs(&n).forest().len(),
        0,
        "the dummy x₀ committed as a no-op; P is not yet issuable"
    );
    assert_certs_match_executed(&n, "boundary 2");

    n.end_window(3, 3 * T);
    assert_eq!(
        n.checkpoint().s.untruncated(),
        vec![Entry::Cmd(a), Entry::Cmd(b), Entry::Cmd(c)]
    );
    assert_eq!(certs(&n).forest().len(), 3);
    assert_certs_match_executed(&n, "boundary 3");
    assert_eq!(
        certs(&n).forest().peak_heights(),
        vec![1, 0],
        "m = 3 decomposes as 2 + 1, so a real merge happened"
    );
    let (prev, last) = certs(&n)
        .last_two(1)
        .expect("client 1 has a committed command");
    assert_eq!((prev.is_none(), last.pos), (true, 0));
}

#[test]
fn only_committed_commands_enter_the_forest_not_nulls_or_nops() {
    // §5's forest is over commands (pp. 27–28): Null bumps sn only and Nop
    // does nothing, exactly as `execute` treats them.
    let a = cmd(1, 1, 11);
    let mut n = node();
    let log = vec![
        timed(Entry::Nop(0), 0),
        timed(Entry::Cmd(a), 0),
        timed(Entry::Null { client: 9, sn: 4 }, 0),
        timed(Entry::Nop(77), 0),
    ];
    n.step(&vec![healthy_reply(&log); 3], &[], 6, &mut rng());
    n.end_window(1, T);
    assert_eq!(n.checkpoint().p.as_deref(), Some(&log[..]), "all aged");

    n.end_window(2, 2 * T);
    assert_eq!(
        n.checkpoint().s.untruncated(),
        vec![Entry::Cmd(a)],
        "only the command commits"
    );
    assert_eq!(certs(&n).forest().len(), 1);
    assert_eq!(certs(&n).forest().roots(), vec![leaf_hash(&Entry::Cmd(a))]);
    assert_certs_match_executed(&n, "mixed P");
    assert_eq!(
        n.checkpoint().s.sn_get(9),
        Some(4),
        "the Null still bumped its client's sn"
    );
    assert!(
        certs(&n).last_two(9).is_none(),
        "a nulled client holds no certifiable command"
    );
}

#[test]
fn rollback_recommits_p_into_the_forest_exactly_once() {
    // The rolled-back checkpoint's forest covers only what it had EXECUTED;
    // its P commits at this very boundary, so P's commands enter the forest
    // here for the first and only time.
    let (a, b, c, d) = (cmd(1, 1, 11), cmd(2, 1, 22), cmd(3, 1, 33), cmd(4, 1, 44));
    let mut n = node();
    let mut lens = Vec::new();

    let first = vec![
        timed(Entry::Nop(0), 0),
        timed(Entry::Cmd(a), 0),
        timed(Entry::Cmd(b), 0),
    ];
    n.step(&vec![healthy_reply(&first); 3], &[], 6, &mut rng());
    n.end_window(1, T);
    lens.push(certs(&n).forest().len());
    n.end_window(2, 2 * T);
    lens.push(certs(&n).forest().len());
    assert_eq!(
        n.checkpoint().s.untruncated(),
        vec![Entry::Cmd(a), Entry::Cmd(b)]
    );
    assert_certs_match_executed(&n, "pre-rollback");

    let second = vec![
        timed(Entry::Nop(2 * T), 2 * T),
        timed(Entry::Cmd(c), 2 * T),
        timed(Entry::Cmd(d), 2 * T),
    ];
    n.step(&vec![healthy_reply(&second); 3], &[], 2 * T + 1, &mut rng());
    n.end_window(3, 3 * T);
    lens.push(certs(&n).forest().len());
    assert_eq!(
        n.checkpoint().p.as_deref(),
        Some(&second[..]),
        "c and d are pre-committed in P, not in the forest"
    );
    assert_eq!(certs(&n).forest().len(), 2, "still only a and b");

    let own = n.log_entries().unwrap().to_vec();
    n.step(
        &vec![reply(Some(own), genesis(), RState::Reset); 3],
        &[],
        3 * T + 1,
        &mut rng(),
    );
    assert_eq!(n.reset_state(), RState::Reset);

    n.end_window(4, 4 * T);
    lens.push(certs(&n).forest().len());
    assert_eq!(
        n.checkpoint().s.untruncated(),
        vec![Entry::Cmd(a), Entry::Cmd(b), Entry::Cmd(c), Entry::Cmd(d)],
        "the restored P re-commits in the same boundary"
    );
    assert_eq!(
        certs(&n).forest().len(),
        4,
        "c and d entered exactly once — no double append across the rollback"
    );
    assert_certs_match_executed(&n, "post-rollback");
    assert!(
        lens.windows(2).all(|w| w[1] >= w[0]),
        "forest length never decreases across a rollback: {lens:?}"
    );
}

// --- §5 certificate state rides the checkpoint but never decides anything ---

#[test]
fn checkpoint_equality_ignores_the_certificate_payload() {
    // Protocol decisions read (S, P, W) only: fork detection, boundary logic
    // and the sim's lineage oracle must not see the cert forest.
    let a = cp(state_of(&[cmd(1, 1, 7)]), Some(seed_log()), 3);
    let mut b = a.clone();
    held_mut(&mut b).append(&Entry::Cmd(cmd(1, 1, 7)));
    assert_ne!(
        held(&a).forest().roots(),
        held(&b).forest().roots(),
        "anti-vacuity: the two cert states must really differ"
    );
    assert_eq!(a, b, "cert payload must not influence a protocol decision");
}

#[test]
fn checkpoint_clone_carries_cert_state_by_value() {
    // Adoption clones a checkpoint and rollback restores one; an aliased
    // forest would couple a live checkpoint to a rolled-back one.
    let mut a = cp(SharedState::default(), None, 0);
    held_mut(&mut a).append(&Entry::Cmd(cmd(1, 1, 7)));
    let before = held(&a).forest().roots();
    let mut b = a.clone();
    held_mut(&mut b).append(&Entry::Cmd(cmd(1, 2, 8)));
    assert_eq!(
        held(&a).forest().roots(),
        before,
        "original forest untouched"
    );
    assert_eq!(held(&a).forest().len(), 1);
    assert_eq!(held(&b).forest().len(), 2);
}

// --- during-window step 4: < ℓ replies (Alg 6, p. 31; Lemma 6.2 forward) ---

#[test]
fn below_ell_recovery_replies_force_bot_r_and_bot_log() {
    let mut n = node();
    let replies = vec![healthy_reply(&seed_log()); 2]; // ℓ = 3
    n.step(&replies, &[], 1, &mut rng());
    assert_eq!(n.reset_state(), RState::Bot);
    assert_eq!(n.log_entries(), None);
    assert_eq!(*n.shared_state(), SharedState::default(), "state untouched");
    assert_eq!(*n.checkpoint(), genesis(), "checkpoint untouched");
}

/// The branches that cannot merge must consume NO randomness at all. This is
/// the sharp edge of the split: `step ≡ draw + step_chosen` cannot see it,
/// because both sides of that comparison call the same draw, so a draw taken
/// where the box takes none would agree with itself and still shift every
/// later draw in the run's single stream.
#[test]
fn a_round_that_cannot_merge_takes_no_draw() {
    use rand::Rng as _;
    let bearing = |i: u64| healthy_reply(&[timed(Entry::Cmd(cmd(i as u32 + 1, 1, i + 10)), 0)]);
    let bare = || reply(None, genesis(), RState::Reset);
    for (with_log, without) in [
        (0usize, 0usize), // no replies at all
        (1, 1),           // under ℓ replies
        (0, 4),           // enough replies, none log-bearing
        (2, 3),           // enough replies, under ℓ log-bearing
    ] {
        let mut replies: Vec<RecoveryReply> = (0..with_log as u64).map(bearing).collect();
        replies.extend((0..without).map(|_| bare()));
        let label = format!("{with_log} log-bearing of {}", replies.len());

        let n = node();
        let mut probe = rng();
        let mut untouched = rng();
        assert!(
            n.draw_reply_choice(&replies, &mut probe).is_none(),
            "no merge is possible, so no choice may be drawn, {label}"
        );
        assert_eq!(
            probe.random::<u64>(),
            untouched.random::<u64>(),
            "the no-draw branch moved the stream, {label}"
        );
    }
}

/// step ≡ draw_reply_choice + step_chosen on EVERY branch, RNG stream position
/// included — the split that lets a driver pre-draw sequentially (preserving
/// the one-stream contract) and apply node steps in parallel.
///
/// The three reply mixes below are the three branches of the Alg 6 box, and
/// two of them take no draw at all: under ℓ replies, and ℓ replies too few of
/// which carry a log. A split that drew unconditionally would keep the same
/// logs and still shift every later draw in the stream, so the position check
/// is the load-bearing assertion here, not the log comparison.
#[test]
fn step_splits_into_draw_and_chosen_apply_exactly() {
    use rand::Rng as _;
    let bearing = |i: u64| healthy_reply(&[timed(Entry::Cmd(cmd(i as u32 + 1, 1, i + 10)), 0)]);
    let bare = || reply(None, genesis(), RState::Reset);
    // (log-bearing, log-less) over the branch boundary at ℓ = 3.
    for (with_log, without) in [
        (0usize, 0usize), // no replies at all → R = ⊥, no draw
        (1, 1),           // 2 replies, under ℓ → R = ⊥, no draw
        (0, 4),           // enough replies, none log-bearing → ⊥ log, no draw
        (2, 3),           // enough replies, under ℓ log-bearing → ⊥ log, no draw
        (3, 0),           // exactly ℓ log-bearing → draws
        (3, 4),           // ℓ log-bearing among more → draws
        (6, 1),           // every log-bearing reply a candidate → draws
    ] {
        let mut replies: Vec<RecoveryReply> = (0..with_log as u64).map(bearing).collect();
        replies.extend((0..without).map(|_| bare()));
        let appends = [(cmd(90, 1, 900), 1u64)];
        let label = format!("{with_log} log-bearing of {}", replies.len());

        let mut whole = node();
        let mut split = node();
        let mut rng_a = rng();
        let mut rng_b = rng();
        whole.step(&replies, &appends, 1, &mut rng_a);
        let chosen = split.draw_reply_choice(&replies, &mut rng_b);
        split.step_chosen(&replies, &appends, chosen.as_deref());

        assert_eq!(whole.log_entries(), split.log_entries(), "log, {label}");
        assert_eq!(whole.reset_state(), split.reset_state(), "R, {label}");
        assert_eq!(
            whole.checkpoint(),
            split.checkpoint(),
            "checkpoint, {label}"
        );
        assert_eq!(whole.shared_state(), split.shared_state(), "state, {label}");
        assert_eq!(
            rng_a.random::<u64>(),
            rng_b.random::<u64>(),
            "stream position, {label}"
        );
    }
}

// --- §4 p. 25 / p. 29 compactness: forgetting the committed prefix ---

/// The line the driver applies: the node's OWN checkpoint state length. Any
/// other line would be a claim about what this node's PEERS hold, which the
/// paper's rule never makes.
fn forget_to_own_checkpoint(n: &mut RecoveryNode) {
    n.forget_committed_prefix(n.checkpoint().s.logical_len());
}

/// The logical sequence is unchanged by forgetting: the length, the committed
/// sequence numbers, and everything still held read exactly as before. §4's
/// triage/ack clause reads `sn_get`, so the sn map surviving is what keeps it
/// correct afterwards — a client's committed sn outlives the entry it came from.
#[test]
fn forgetting_moves_no_logical_length_and_no_committed_sequence_number() {
    let (a, b) = (cmd(1, 1, 77), cmd(2, 4, 88));
    let mut state = state_of(&[a, b]);
    let before: Vec<(u32, u64)> = state.sn_iter().collect();
    state.forget_committed_prefix(1);

    assert_eq!(state.logical_len(), 2);
    assert_eq!(state.executed_offset(), 1);
    assert!(
        state
            .executed()
            .iter_from(state.executed_offset())
            .eq([Entry::Cmd(b)].iter())
    );
    assert_eq!(state.sn_iter().collect::<Vec<_>>(), before);
    assert_eq!(state.sn_get(1), Some(1), "the entry is gone, the sn is not");
    assert_eq!(state.sn_get(2), Some(4));
}

/// Between-window step 1: a node that has forgotten still rolls back
/// correctly, because the rollback REPLACES the state wholesale from the
/// checkpoint rather than reading into it.
#[test]
fn a_node_that_forgot_still_rolls_back_wholesale_to_its_checkpoint() {
    let mut n = node();
    let a = cmd(1, 1, 77);
    // A drained-but-healthy checkpoint: P is empty rather than ⊥, which is
    // legitimate for any W ≥ 1 and is what lets step 2 run without
    // re-executing anything already in S.
    let cp2 = cp(state_of(&[a]), Some(Vec::new()), 2);
    n.step(
        &vec![reply(Some(seed_log()), cp2.clone(), RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    forget_to_own_checkpoint(&mut n);
    assert_eq!(n.shared_state().executed_offset(), 1, "the forget landed");
    assert_eq!(
        n.shared_state().logical_len(),
        n.shared_state().executed_offset(),
        "nothing live remains"
    );

    // Replies that carry a log but no `no-reset` and no newer checkpoint:
    // R converges to reset, the log survives, nothing is adopted.
    let peer = vec![timed(Entry::Nop(9), 9)];
    n.step(
        &vec![reply(Some(peer), cp2.clone(), RState::Reset); 3],
        &[],
        5,
        &mut rng(),
    );
    assert_eq!(n.reset_state(), RState::Reset);
    n.end_window(3, 30);

    assert_eq!(
        n.shared_state().logical_len(),
        1,
        "the rollback restores the checkpoint's LENGTH, whatever was forgotten"
    );
    assert_eq!(
        n.shared_state().executed_offset(),
        0,
        "and its representation: the checkpoint held the whole prefix"
    );
    assert_eq!(*n.shared_state(), *cp2.s);
}

/// In-window step 3: adoption also replaces the state wholesale, so a node
/// that has forgotten adopts cleanly — and the adopted state arrives carrying
/// the DONOR's forget line as its own offset. That is how two nodes reach
/// equal logical states with unequal offsets in the healthy mainline.
#[test]
fn adoption_replaces_a_forgotten_state_and_carries_the_donors_line() {
    let (a, b) = (cmd(1, 1, 77), cmd(2, 1, 88));
    let mut donor_state = state_of(&[a, b]);
    donor_state.forget_committed_prefix(2);
    let cp4 = cp(donor_state, Some(vec![timed(Entry::Cmd(b), 7)]), 4);

    let mut n = node();
    let cp2 = cp(state_of(&[a]), Some(vec![timed(Entry::Cmd(a), 3)]), 2);
    n.step(
        &vec![reply(Some(seed_log()), cp2, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    forget_to_own_checkpoint(&mut n);

    n.step(
        &vec![reply(Some(seed_log()), cp4, RState::NoReset); 3],
        &[],
        8,
        &mut rng(),
    );
    assert_eq!(n.shared_state().logical_len(), 2);
    assert_eq!(
        n.shared_state().executed_offset(),
        2,
        "the adopted state carries the donor's line, not the adopter's"
    );
    assert_eq!(n.checkpoint().w, 4);
}

/// Between-window step 2 commits P onto S. `execute` only ever appends, so a
/// forgotten prefix cannot disturb it — and P stays a literal prefix of the
/// LOG across a forget, because P and the executed history are different
/// sequences and only one of them lost anything.
#[test]
fn committing_p_after_a_forget_appends_and_leaves_p_a_log_prefix() {
    let mut n = node();
    let (a, b) = (cmd(1, 1, 77), cmd(2, 1, 88));
    let pre = vec![timed(Entry::Cmd(a), 3)];
    let cp2 = cp(SharedState::default(), Some(pre.clone()), 2);
    n.step(
        &vec![reply(Some(seed_log()), cp2, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    // Extend the log past P so the strip leaves something behind.
    let extended = vec![timed(Entry::Cmd(a), 3), timed(Entry::Cmd(b), 5)];
    n.step(&vec![healthy_reply(&extended); 3], &[], 6, &mut rng());
    n.end_window(3, 20);
    assert_eq!(n.shared_state().logical_len(), 1, "P committed");

    forget_to_own_checkpoint(&mut n);
    assert_eq!(n.shared_state().executed_offset(), 1);

    let p2 = n.checkpoint().p.clone().expect("minted");
    assert!(
        n.log_entries().expect("log").starts_with(&p2),
        "P must stay a literal prefix of L across a forget"
    );
    let commits = p2
        .iter()
        .filter(|t| matches!(t.entry, Entry::Cmd(_)))
        .count() as u64;
    n.end_window(4, 30);
    assert_eq!(
        n.shared_state().logical_len(),
        1 + commits,
        "the next commit appended onto a state whose front is gone"
    );
    assert_eq!(
        n.shared_state().executed_offset(),
        1,
        "the front stays gone"
    );
}

/// The residency claim at the primitive: forgetting bounds what is PHYSICALLY
/// held. The logical length grows forever, so only the buffer can witness it,
/// and the amortized compaction is what keeps the dead prefix from being the
/// leak the forget was supposed to remove.
#[test]
fn forgetting_bounds_the_physical_buffer_not_the_logical_length() {
    let entries: Vec<Entry> = (0..200u64)
        .map(|op| Entry::Cmd(cmd(op as u32, 1, op)))
        .collect();
    let whole = SharedState::from_entries(entries.clone(), Default::default());
    let mut forgotten = SharedState::from_entries(entries, Default::default());
    for line in 1..=196 {
        forgotten.forget_committed_prefix(line);
    }

    assert_eq!(whole.logical_len(), forgotten.logical_len());
    assert_eq!(whole.retained_len(), 200);
    assert!(
        forgotten.retained_len() <= 8,
        "the dead prefix must be compacted out, not accumulated: {}",
        forgotten.retained_len()
    );
    assert_eq!(
        forgotten.logical_len() - forgotten.executed_offset(),
        4,
        "and the live tail is exactly what was not forgotten"
    );
}

// --- sharing a checkpoint across unequal forget lines (H1) ---

/// The pair interning exists for: equal logical content, different forget
/// lines. `SharedState`'s equality ASSERTS that equal-length states agree on
/// what was forgotten — a real compact-side tripwire that stays — so the swap
/// has to go through a surface that never compares the states themselves.
fn h1_pair() -> (Checkpoint, Checkpoint) {
    let (a, b, c) = (cmd(1, 1, 11), cmd(2, 1, 22), cmd(3, 1, 33));
    let pre = Some(vec![timed(Entry::Cmd(a), 2)]);
    let mine = cp(state_of(&[a, b, c]), pre.clone(), 4);
    let mut theirs = cp(state_of(&[a, b, c]), pre, 4);
    std::sync::Arc::make_mut(&mut theirs.s).forget_committed_prefix(2);
    (mine, theirs)
}

#[test]
fn a_checkpoint_is_shared_across_unequal_forget_lines() {
    let (mine, theirs) = h1_pair();
    assert_ne!(
        mine.s.executed().offset(),
        theirs.s.executed().offset(),
        "anti-vacuity: the pair must really disagree on what was forgotten"
    );
    let mut n = node();
    n.step(
        &vec![reply(Some(seed_log()), mine, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    assert_eq!(n.checkpoint().w, 4, "the node adopted the pair's half");

    let shared = std::sync::Arc::new(theirs);
    n.share_checkpoint(&shared);
    assert!(
        std::sync::Arc::ptr_eq(&n.checkpoint_shared(), &shared),
        "the node must hold the representative's allocation, not a copy"
    );
    assert_eq!(n.checkpoint().s.logical_len(), 3);
    assert_eq!(
        n.checkpoint().s.executed().offset(),
        2,
        "and it carries the representative's forget line, as adoption already does"
    );
}

#[test]
#[should_panic(expected = "share_checkpoint over states of unequal length")]
fn sharing_refuses_a_checkpoint_of_another_length() {
    let (mine, _) = h1_pair();
    let mut n = node();
    n.step(
        &vec![reply(Some(seed_log()), mine, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    let other = cp(state_of(&[cmd(1, 1, 11)]), n.checkpoint().p.clone(), 4);
    n.share_checkpoint(&std::sync::Arc::new(other));
}

#[test]
#[should_panic(expected = "share_checkpoint over unequal P")]
fn sharing_refuses_a_checkpoint_with_another_prefix() {
    let (mine, mut theirs) = h1_pair();
    let mut n = node();
    n.step(
        &vec![reply(Some(seed_log()), mine, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    theirs.p = None;
    n.share_checkpoint(&std::sync::Arc::new(theirs));
}

#[test]
#[should_panic(expected = "share_checkpoint across windows")]
fn sharing_refuses_a_checkpoint_of_another_window() {
    let (mine, mut theirs) = h1_pair();
    let mut n = node();
    n.step(
        &vec![reply(Some(seed_log()), mine, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    theirs.w = 5;
    n.share_checkpoint(&std::sync::Arc::new(theirs));
}

/// A Null bumps sn(c) without extending S, so two states of equal length and equal executed
/// entries can still disagree on the sn map that the client rule reads.
#[test]
#[should_panic(expected = "share_checkpoint over unequal committed sequence numbers")]
fn sharing_refuses_a_checkpoint_with_another_sn_map() {
    let (mine, _) = h1_pair();
    let mut n = node();
    n.step(
        &vec![reply(Some(seed_log()), mine, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    let (a, b, c) = (cmd(1, 1, 11), cmd(2, 1, 22), cmd(3, 1, 33));
    let bumped = SharedState::from_entries(
        [a, b, c].iter().map(|x| Entry::Cmd(*x)).collect(),
        [(1, 1), (2, 1), (3, 2)].into(),
    );
    n.share_checkpoint(&std::sync::Arc::new(cp(
        bumped,
        n.checkpoint().p.clone(),
        4,
    )));
}

/// The input no digest covers: a fork signature is computed over (S, P, W) and
/// says nothing about whether §5 is mounted, so a mixed fleet — a caller
/// convention this type does not enforce — could otherwise lose the layer
/// silently on the node that had it.
#[test]
#[should_panic(expected = "share_checkpoint across the §5 gate")]
fn sharing_refuses_a_checkpoint_across_the_certificate_gate() {
    let (mine, mut theirs) = h1_pair();
    let mut n = node();
    n.step(
        &vec![reply(Some(seed_log()), mine, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    assert!(
        n.checkpoint().certs.is_some(),
        "anti-vacuity: this node must be carrying the layer"
    );
    theirs.certs = None;
    n.share_checkpoint(&std::sync::Arc::new(theirs));
}

/// A digest-keyed caller proves (S, P, W) equal; two states of equal length and sn map that
/// differ in one executed entry are what a hash collision would hand over.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "share_checkpoint over states whose retained entries differ")]
fn sharing_refuses_a_checkpoint_whose_executed_content_differs() {
    let (mine, _) = h1_pair();
    let mut n = node();
    n.step(
        &vec![reply(Some(seed_log()), mine, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    let (a, b, forged) = (cmd(1, 1, 11), cmd(2, 1, 22), cmd(3, 1, 34));
    let theirs = cp(state_of(&[a, b, forged]), n.checkpoint().p.clone(), 4);
    n.share_checkpoint(&std::sync::Arc::new(theirs));
}

#[test]
#[should_panic(expected = "share_checkpoint over unequal §5 forests")]
fn sharing_refuses_a_checkpoint_with_another_certificate_forest() {
    let (mine, mut theirs) = h1_pair();
    let mut n = node();
    n.step(
        &vec![reply(Some(seed_log()), mine, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    held_mut(&mut theirs).append(&Entry::Cmd(cmd(9, 1, 99)));
    n.share_checkpoint(&std::sync::Arc::new(theirs));
}

// --- the boundary mint, shared across a proven-equal class ---

/// Three nodes given identical boundary inputs: one mints privately, one takes
/// the shared mint, one mints privately as the control. The shared node must
/// end where the private ones end — same (S, P, W), same log, same R, same §5
/// forest — because the checkpoint the paper defines is a function of those
/// inputs and nothing else.
#[test]
fn the_shared_mint_leaves_a_node_where_a_private_mint_would_have() {
    let (a, b, c) = (cmd(1, 1, 11), cmd(2, 1, 22), cmd(3, 1, 33));
    let log = vec![
        timed(Entry::Nop(0), 0),
        timed(Entry::Cmd(a), 2),
        timed(Entry::Cmd(b), 2),
        timed(Entry::Cmd(c), 2),
    ];
    let (mut rep, mut member, mut control) = (node(), node(), node());
    for n in [&mut rep, &mut member, &mut control] {
        n.step(&vec![healthy_reply(&log); 3], &[], 6, &mut rng());
    }
    for w in 1..=3 {
        rep.end_window(w, w * T);
        member.end_window_shared(w, w * T, &rep.checkpoint_shared());
        control.end_window(w, w * T);
    }

    assert!(
        std::sync::Arc::ptr_eq(&rep.checkpoint_shared(), &member.checkpoint_shared()),
        "the member must hold the class allocation, not a copy"
    );
    assert!(
        !std::sync::Arc::ptr_eq(&control.checkpoint_shared(), &member.checkpoint_shared()),
        "anti-vacuity: the control must have minted its own"
    );
    assert_eq!(*member.checkpoint(), *control.checkpoint());
    assert_eq!(member.log_entries(), control.log_entries());
    assert_eq!(member.reset_state(), control.reset_state());
    assert_eq!(
        member.shared_state().untruncated(),
        control.shared_state().untruncated()
    );
    assert_eq!(certs(&control).forest().len(), 3, "anti-vacuity");
    assert_eq!(
        certs(&member).forest().roots(),
        certs(&control).forest().roots()
    );
}

/// The rolling-back half of the same statement: step 1 restores the state and
/// the log from the checkpoint, so a class member still rolls back on its own
/// before installing the shared result.
#[test]
fn the_shared_mint_still_rolls_a_resetting_member_back_first() {
    let a = cmd(1, 1, 11);
    let log = vec![timed(Entry::Nop(0), 0), timed(Entry::Cmd(a), 2)];
    let (mut rep, mut control) = (node(), node());
    for n in [&mut rep, &mut control] {
        n.step(&vec![healthy_reply(&log); 3], &[], 6, &mut rng());
        n.end_window(1, T);
        // All-reset replies with no log: R goes Reset, the log is dropped, and
        // the boundary must restore both from the checkpoint.
        n.step(
            &vec![reply(None, genesis(), RState::Reset); 3],
            &[],
            T + 1,
            &mut rng(),
        );
    }
    let mut member = node();
    member.step(&vec![healthy_reply(&log); 3], &[], 6, &mut rng());
    member.end_window(1, T);
    member.step(
        &vec![reply(None, genesis(), RState::Reset); 3],
        &[],
        T + 1,
        &mut rng(),
    );
    assert_eq!(member.reset_state(), RState::Reset, "the setup must reset");

    rep.end_window(2, 2 * T);
    control.end_window(2, 2 * T);
    member.end_window_shared(2, 2 * T, &rep.checkpoint_shared());

    assert_eq!(member.reset_state(), RState::NoReset);
    assert_eq!(*member.checkpoint(), *control.checkpoint());
    assert_eq!(member.log_entries(), control.log_entries());
    assert_eq!(
        member.shared_state().untruncated(),
        control.shared_state().untruncated()
    );
}

/// Sound-over-complete has a safety net: a key that claimed equality it did
/// not have is a panic, not a silent substitution. One extra log entry is
/// enough to move P.
#[test]
#[should_panic(expected = "end_window_shared over an unequal P")]
fn the_shared_mint_refuses_a_member_whose_log_differs() {
    let (a, b) = (cmd(1, 1, 11), cmd(2, 1, 22));
    let short = vec![timed(Entry::Nop(0), 0), timed(Entry::Cmd(a), 2)];
    let long = vec![
        timed(Entry::Nop(0), 0),
        timed(Entry::Cmd(a), 2),
        timed(Entry::Cmd(b), 2),
    ];
    let mut rep = node();
    rep.step(&vec![healthy_reply(&long); 3], &[], 6, &mut rng());
    let mut member = node();
    member.step(&vec![healthy_reply(&short); 3], &[], 6, &mut rng());
    for w in 1..=2 {
        rep.end_window(w, w * T);
    }
    member.end_window(1, T);
    member.end_window_shared(2, 2 * T, &rep.checkpoint_shared());
}

// --- opt-in execute-once guard: a P entry whose slot sn(c) already covers ---

/// Adopts a checkpoint whose P re-carries a command S already executed, then commits P.
fn commit_a_respread_command(policy: RepeatedCommit) -> (SharedState, u64) {
    let a = cmd(1, 1, 77);
    let adopted = Checkpoint {
        s: state_of(&[a]).into(),
        p: Some(vec![timed(Entry::Cmd(a), 3)]),
        w: 2,
        certs: None,
    };
    let mut n = RecoveryNode::new(Config::default(), T, false).with_repeated_commit(policy);
    n.step(
        &vec![reply(Some(seed_log()), adopted, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    n.end_window(3, 30);
    (n.shared_state().clone(), n.repeat_skips())
}

#[test]
fn a_respread_command_in_p_executes_again_by_default() {
    let a = cmd(1, 1, 77);
    let (state, skips) = commit_a_respread_command(RepeatedCommit::Execute);
    assert_eq!(
        state.executed().to_vec(),
        vec![Entry::Cmd(a), Entry::Cmd(a)]
    );
    assert_eq!(skips, 0);
}

#[test]
fn skip_commits_a_respread_command_in_p_once() {
    let a = cmd(1, 1, 77);
    let (state, skips) = commit_a_respread_command(RepeatedCommit::Skip);
    assert_eq!(state, state_of(&[a]));
    assert_eq!(skips, 1);
}

/// Adopts a checkpoint whose P holds client 1's sn 2 while S has no commit of client 1.
fn adopt_an_sn_gap() -> RecoveryNode {
    let adopted = Checkpoint {
        s: SharedState::default().into(),
        p: Some(vec![timed(Entry::Cmd(cmd(1, 2, 78)), 3)]),
        w: 2,
        certs: None,
    };
    let mut n =
        RecoveryNode::new(Config::default(), T, false).with_repeated_commit(RepeatedCommit::Skip);
    n.step(
        &vec![reply(Some(seed_log()), adopted, RState::NoReset); 3],
        &[],
        4,
        &mut rng(),
    );
    n
}

#[test]
fn skip_refuses_an_out_of_sn_order_p_at_the_boundary_and_keeps_the_checkpoint() {
    let mut n = adopt_an_sn_gap();
    let before = n.checkpoint_shared();
    n.end_window(3, 30);
    assert_eq!(
        n.sn_order_violation(),
        Some(OutOfSnOrder {
            client: 1,
            sn: 2,
            committed_sn: 0
        })
    );
    assert_eq!(
        n.shared_state(),
        &SharedState::default(),
        "S_i is unchanged"
    );
    assert!(
        Arc::ptr_eq(&n.checkpoint_shared(), &before),
        "no checkpoint is minted"
    );
}

#[test]
#[should_panic(expected = "refused an out-of-sn-order commit")]
fn a_recovery_node_that_refused_a_commit_is_not_stepped_again() {
    let mut n = adopt_an_sn_gap();
    n.end_window(3, 30);
    n.step(&[], &[], 31, &mut rng());
}

#[test]
#[should_panic(expected = "not combined with the §5 certificate layer")]
fn skip_refuses_a_node_that_mounts_certificates() {
    let _ = node().with_repeated_commit(RepeatedCommit::Skip);
}

// An adopting node takes P' over into L_i stamps included (Alg 6 "If W' > W"), so two checkpoints whose P
// differ only in stamps are not interchangeable and `==` keeps them apart.
#[test]
fn checkpoint_equality_compares_p_with_its_stamps() {
    let x = Entry::Cmd(cmd(1, 1, 7));
    let at = |round| {
        cp(
            SharedState::default(),
            Some(vec![timed(x.clone(), round)]),
            3,
        )
    };
    assert_eq!(at(32), at(32));
    assert_ne!(at(32), at(33));
}
