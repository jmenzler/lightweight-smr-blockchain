use protocol::Config;
use protocol::chunked::ChunkSeq;
use protocol::compact::{
    ClientCommand, CompactNode, CompactReply, Entry, SharedState, Timed, Triage,
};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use rstest::rstest;

const T: u64 = 10;

fn rng() -> ChaCha12Rng {
    ChaCha12Rng::seed_from_u64(42)
}

fn node() -> CompactNode {
    CompactNode::new(Config::default(), T)
}

fn cmd(client: u32, sn: u64, op: u64) -> ClientCommand {
    ClientCommand { client, sn, op }
}

fn timed(entry: Entry, round: u64) -> Timed {
    Timed { entry, round }
}

/// A reply whose log holds the given entries, all received at `round`; no state attached.
fn log_reply(entries: &[Entry], round: u64) -> CompactReply {
    CompactReply::from_log(
        std::sync::Arc::new(entries.iter().map(|e| timed(e.clone(), round)).collect()),
        None,
    )
}

// --- replying to requests (Algorithm 5, step 2) ---

#[rstest]
#[case::requester_healthy(false)]
#[case::requester_recovering(true)]
fn a_server_with_a_log_replies_with_it_and_attaches_its_state_iff_b_is_set(#[case] b_j: bool) {
    // "if L_i ≠ ⊥ then for any request ... i sends L_i to j, and if b_j = 1 then also S_i"
    let mut n = node();
    n.step(
        &vec![log_reply(&[Entry::Cmd(cmd(1, 1, 77))], 0); 3],
        &[],
        T,
        &mut rng(),
    );
    let reply = n.answer(b_j).expect("L_i ≠ ⊥ answers");
    assert_eq!(reply.l_j.entries().iter().count(), 1);
    assert_eq!(
        reply
            .s_j
            .as_ref()
            .map(|s| s.untruncated().iter().cloned().collect::<Vec<_>>()),
        b_j.then(|| vec![Entry::Cmd(cmd(1, 1, 77))]),
        "S_i travels exactly when b_j = 1"
    );
}

#[rstest]
#[case::requester_healthy(false)]
#[case::requester_recovering(true)]
fn a_bottom_server_sends_no_reply(#[case] b_j: bool) {
    let mut n = node();
    n.step(&[], &[], 1, &mut rng());
    assert_eq!(n.log_entries(), None);
    assert_eq!(n.answer(b_j), None);
}

// --- client triage (Algorithm 5, step 1) ---

#[rstest]
#[case::next_sn_amplifies(1, Triage::Amplify)]
#[case::committed_sn_acks(0, Triage::AckCommitted)]
#[case::future_sn_ignored(3, Triage::Ignore)]
fn triage_by_sequence_number(#[case] sn: u64, #[case] expected: Triage) {
    // fresh node: sn(c) = 0 for every client
    assert_eq!(node().on_client_command(&cmd(1, sn, 77)), expected);
}

#[test]
fn command_already_in_log_is_ignored() {
    let mut n = node();
    n.step(
        &vec![log_reply(&[Entry::Cmd(cmd(1, 1, 77))], 0); 3],
        &[],
        1,
        &mut rng(),
    );
    assert_eq!(n.on_client_command(&cmd(1, 1, 77)), Triage::Ignore);
}

/// Pins the merge's membership semantics end to end: the median log is the
/// base, the union walks the lexicographically sorted logs appending each
/// entry on FIRST occurrence only, appends dedupe against everything before
/// them, and every survivor keeps that first occurrence's position.
#[test]
fn merge_appends_first_occurrences_only_in_sorted_log_order() {
    let (a, b, c, d, e) = (
        Entry::Cmd(cmd(1, 1, 11)),
        Entry::Cmd(cmd(2, 1, 22)),
        Entry::Cmd(cmd(3, 1, 33)),
        Entry::Cmd(cmd(4, 1, 44)),
        Entry::Cmd(cmd(5, 1, 55)),
    );
    let mut n = node();
    n.step(
        &[
            log_reply(&[a.clone(), b.clone()], 0),
            log_reply(&[b.clone(), c.clone()], 0),
            log_reply(&[a.clone(), d.clone()], 0),
        ],
        &[(cmd(3, 1, 33), 1), (cmd(5, 1, 55), 1)],
        1,
        &mut rng(),
    );
    // Sorted logs: [a,b] < [a,d] < [b,c] → median [a,d] is the base; the
    // union then contributes b (from [a,b]) and c (from [b,c]); append c is
    // already present, append e is fresh.
    let got: Vec<Entry> = n
        .log_entries()
        .expect("non-⊥")
        .iter()
        .map(|t| t.entry.clone())
        .collect();
    assert_eq!(got, [a, d, b, c, e]);
}

/// A median log can genuinely carry duplicate identical entries — several
/// conflicting commands from one client become the same ⊥ at different
/// positions. The membership index must keep the median's duplicates yet
/// refuse a third copy from the union: the one input class where a set and
/// a sorted vector internally differ.
#[test]
fn a_median_with_duplicate_nulls_keeps_them_and_admits_no_more() {
    let null = Entry::Null { client: 1, sn: 1 };
    let (a, b) = (Entry::Cmd(cmd(2, 1, 22)), Entry::Cmd(cmd(3, 1, 33)));
    let mut n = node();
    n.step(
        &[
            log_reply(&[a.clone(), b.clone()], 0),
            log_reply(&[null.clone(), a.clone(), null.clone()], 0),
            log_reply(&[null.clone(), b.clone()], 0),
        ],
        &[],
        1,
        &mut rng(),
    );
    // Sorted logs: [a,b] < [⊥,a,⊥] < [⊥,b] → the duplicate-bearing log is
    // the median base; the union may add b but neither a fourth ⊥ nor a.
    let got: Vec<Entry> = n
        .log_entries()
        .expect("non-⊥")
        .iter()
        .map(|t| t.entry.clone())
        .collect();
    assert_eq!(got, [null.clone(), a, null, b]);
}

/// step ≡ draw_reply_choice + step_chosen on both branches, RNG stream
/// position included — the split that lets a driver pre-draw sequentially
/// (preserving the one-stream contract) and apply node steps in parallel.
#[test]
fn step_splits_into_draw_and_chosen_apply_exactly() {
    use rand::Rng as _;
    for reply_count in [0usize, 1, 2, 3, 5, 9] {
        let entries: Vec<Vec<Entry>> = (0..reply_count)
            .map(|j| vec![Entry::Cmd(cmd(j as u32 + 1, 1, j as u64 + 10))])
            .collect();
        let replies: Vec<CompactReply> = entries.iter().map(|e| log_reply(e, 0)).collect();
        let appends = [(cmd(90, 1, 900), 1u64)];
        let mut whole = node();
        let mut split = node();
        let mut rng_a = rng();
        let mut rng_b = rng();
        whole.step(&replies, &appends, 1, &mut rng_a);
        let chosen = split.draw_reply_choice(replies.len(), &mut rng_b);
        split.step_chosen(&replies, &appends, 1, chosen.as_deref());
        assert_eq!(
            whole.log_entries(),
            split.log_entries(),
            "log, {reply_count} replies"
        );
        assert_eq!(
            whole.shared_state(),
            split.shared_state(),
            "state, {reply_count} replies"
        );
        assert_eq!(
            rng_a.random::<u64>(),
            rng_b.random::<u64>(),
            "stream position, {reply_count} replies"
        );
    }
}

#[test]
fn undecided_node_ignores_client_commands() {
    let mut n = node();
    n.step(&[], &[], 1, &mut rng()); // no replies → ⊥
    assert_eq!(n.on_client_command(&cmd(1, 1, 77)), Triage::Ignore);
}

// --- recovery bit b_i (step 2) ---

#[test]
fn b_i_is_set_exactly_when_the_log_is_bot() {
    let mut n = node();
    assert!(!n.b_i());
    n.step(&[], &[], 1, &mut rng());
    assert!(n.b_i());
}

// --- commit-after-T (step 4, aged prefix) ---

#[test]
fn aged_prefix_executes_in_order_and_leaves_the_log() {
    let mut n = node();
    let entries = [Entry::Cmd(cmd(1, 1, 77)), Entry::Cmd(cmd(2, 1, 88))];
    n.step(&vec![log_reply(&entries, 0); 3], &[], T, &mut rng());
    let state = n.shared_state();
    assert_eq!(
        state.untruncated(),
        entries.to_vec(),
        "aged commands execute in log order"
    );
    assert_eq!(state.sn_get(1), Some(1));
    assert_eq!(state.sn_get(2), Some(1));
    let log = n.log_entries().unwrap();
    assert!(
        !log.iter().any(|t| matches!(t.entry, Entry::Cmd(_))),
        "committed commands must leave the log"
    );
}

#[test]
fn young_command_stays_in_log_unexecuted() {
    let mut n = node();
    let reply = CompactReply::from_log(
        ChunkSeq::shared(vec![
            timed(Entry::Cmd(cmd(1, 1, 77)), 0),     // age T → aged
            timed(Entry::Cmd(cmd(2, 1, 88)), T - 1), // age 1 → young
        ]),
        None,
    );
    n.step(&[reply.clone(), reply.clone(), reply], &[], T, &mut rng());
    assert_eq!(
        n.shared_state().untruncated(),
        vec![Entry::Cmd(cmd(1, 1, 77))]
    );
    assert!(
        n.log_entries()
            .unwrap()
            .iter()
            .any(|t| t.entry == Entry::Cmd(cmd(2, 1, 88)))
    );
}

#[test]
fn young_head_blocks_aged_tail() {
    // P_i is the largest PREFIX of aged commands — an aged command behind a
    // young one must not execute
    let mut n = node();
    let reply = CompactReply::from_log(
        ChunkSeq::shared(vec![
            timed(Entry::Cmd(cmd(1, 1, 77)), T - 1), // young head
            timed(Entry::Cmd(cmd(2, 1, 88)), 0),     // aged tail
        ]),
        None,
    );
    n.step(&[reply.clone(), reply.clone(), reply], &[], T, &mut rng());
    assert_eq!(n.shared_state().untruncated(), Vec::<Entry>::new());
}

#[test]
fn emptied_log_gets_a_dummy_command() {
    let mut n = node();
    n.step(
        &vec![log_reply(&[Entry::Cmd(cmd(1, 1, 77))], 0); 3],
        &[],
        T,
        &mut rng(),
    );
    // The command is aged (round T − 0 ≥ T), executes, and empties the log; the paper does not
    // fix x_d, the implementation appends the no-op stamped with the current round.
    assert_eq!(
        n.shared_state().untruncated(),
        vec![Entry::Cmd(cmd(1, 1, 77))]
    );
    assert_eq!(
        n.log_entries().unwrap().to_vec(),
        vec![timed(Entry::Nop(T), T)],
        "the emptied log is exactly (x_d)"
    );
}

#[test]
fn committed_command_acks_the_resending_client() {
    let mut n = node();
    n.step(
        &vec![log_reply(&[Entry::Cmd(cmd(1, 1, 77))], 0); 3],
        &[],
        T,
        &mut rng(),
    );
    assert_eq!(n.on_client_command(&cmd(1, 1, 77)), Triage::AckCommitted);
}

/// Executes `entries` on a fresh node: three equal replies stamped 0, stepped at round T.
fn node_that_committed(entries: &[Entry]) -> CompactNode {
    let mut n = node();
    n.step(&vec![log_reply(entries, 0); 3], &[], T, &mut rng());
    n
}

#[test]
fn a_stale_sequence_number_is_ignored_not_acked() {
    let n = node_that_committed(&[Entry::Cmd(cmd(1, 1, 77)), Entry::Cmd(cmd(1, 2, 78))]);
    assert_eq!(n.shared_state().sn_get(1), Some(2));
    assert_eq!(n.on_client_command(&cmd(1, 1, 77)), Triage::Ignore);
    assert_eq!(n.on_client_command(&cmd(1, 2, 78)), Triage::AckCommitted);
}

#[test]
fn a_bot_server_does_not_ack_a_committed_sequence_number() {
    let mut n = node_that_committed(&[Entry::Cmd(cmd(1, 1, 77))]);
    assert_eq!(n.on_client_command(&cmd(1, 1, 77)), Triage::AckCommitted);
    n.step(&[], &[], T + 1, &mut rng());
    assert_eq!(n.shared_state().sn_get(1), Some(1));
    assert_eq!(n.on_client_command(&cmd(1, 1, 77)), Triage::Ignore);
}

#[test]
fn append_requests_are_lost_with_the_log_below_ell_replies() {
    let mut n = node();
    n.step(
        &vec![log_reply(&[Entry::Cmd(cmd(1, 1, 1))], 0); 2],
        &[(cmd(2, 1, 9), 1)],
        1,
        &mut rng(),
    );
    assert!(n.b_i(), "below ℓ replies the log is ⊥");
    n.step(
        &vec![log_reply(&[Entry::Cmd(cmd(1, 1, 1))], 0); 3],
        &[],
        2,
        &mut rng(),
    );
    let logged: Vec<Entry> = n
        .log_entries()
        .unwrap()
        .iter()
        .map(|t| t.entry.clone())
        .collect();
    assert_eq!(
        logged,
        [Entry::Cmd(cmd(1, 1, 1))],
        "the lost append must not resurface"
    );
}

#[test]
fn a_command_held_only_by_the_server_itself_is_dropped() {
    // L̄ draws on the chosen replies and the appends; the server's own previous log counts only if it was drawn.
    let mut n = node();
    let seed_only = vec![log_reply(&[Entry::Nop(0)], 0); 3];
    n.step(&seed_only, &[(cmd(1, 1, 5), 1)], 1, &mut rng());
    assert!(
        n.log_entries()
            .unwrap()
            .iter()
            .any(|t| t.entry == Entry::Cmd(cmd(1, 1, 5)))
    );
    n.step(&seed_only, &[], 2, &mut rng());
    assert!(
        !n.log_entries()
            .unwrap()
            .iter()
            .any(|t| t.entry == Entry::Cmd(cmd(1, 1, 5)))
    );
}

/// Alg 5's client bullet tests `x ∉ L_i` by command, so a ⊥ for the slot does not stop x being accepted again.
#[test]
fn a_bot_for_the_slot_does_not_block_the_command_and_both_commit() {
    let mut n = node();
    let conflict = [
        log_reply(&[Entry::Cmd(cmd(1, 1, 1))], 5),
        log_reply(&[Entry::Cmd(cmd(1, 1, 2))], 5),
        log_reply(&[Entry::Cmd(cmd(2, 1, 3))], 5),
    ];
    n.step(&conflict, &[], 6, &mut rng());
    assert_eq!(n.on_client_command(&cmd(1, 1, 1)), Triage::Amplify);

    let null = Entry::Null { client: 1, sn: 1 };
    let n = node_that_committed(&[null, Entry::Cmd(cmd(1, 1, 7))]);
    assert_eq!(
        n.shared_state().untruncated(),
        vec![Entry::Cmd(cmd(1, 1, 7))],
        "x executes beside the ⊥ of its slot"
    );
}

// --- duplicate replacement AFTER median-merge (step 4 order, box p.26) ---

#[test]
fn duplicates_nulled_in_merged_log_not_before_median() {
    // A=[(c1,1,opX)] < B=[(c1,1,opY)] < C=[(c2,1,opZ)] lexicographically
    // (opX=1 < opY=2). Paper order: median = B, merge L̄ = [opX-cmd, opZ-cmd],
    // THEN null the (c1,1) pair inside the merged log. Pre-median nulling
    // would instead make [⊥] the median — a different result.
    let mut n = node();
    let replies = [
        log_reply(&[Entry::Cmd(cmd(1, 1, 1))], 5),
        log_reply(&[Entry::Cmd(cmd(1, 1, 2))], 5),
        log_reply(&[Entry::Cmd(cmd(2, 1, 3))], 5),
    ];
    n.step(&replies, &[], 6, &mut rng());
    let entries: Vec<Entry> = n
        .log_entries()
        .unwrap()
        .iter()
        .map(|t| t.entry.clone())
        .collect();
    assert_eq!(
        entries,
        vec![
            Entry::Null { client: 1, sn: 1 },
            Entry::Null { client: 1, sn: 1 },
            Entry::Cmd(cmd(2, 1, 3)),
        ]
    );
}

#[test]
fn a_three_way_conflict_nulls_every_occurrence_including_the_first() {
    // Three distinct ops on one (client, sn) — the conflict list sees the
    // pair twice, but every occurrence (the first op included) must null.
    let mut n = node();
    let replies = [
        log_reply(&[Entry::Cmd(cmd(1, 1, 1)), Entry::Cmd(cmd(2, 1, 9))], 5),
        log_reply(&[Entry::Cmd(cmd(1, 1, 2))], 5),
        log_reply(&[Entry::Cmd(cmd(1, 1, 3))], 5),
    ];
    n.step(&replies, &[], 6, &mut rng());
    let entries: Vec<Entry> = n
        .log_entries()
        .unwrap()
        .iter()
        .map(|t| t.entry.clone())
        .collect();
    let nulls = entries
        .iter()
        .filter(|e| matches!(e, Entry::Null { client: 1, sn: 1 }))
        .count();
    assert_eq!(nulls, 3, "all three conflicting occurrences null");
    assert!(
        entries.contains(&Entry::Cmd(cmd(2, 1, 9))),
        "the unconflicted command survives"
    );
}

#[test]
fn committed_null_still_advances_the_client_sequence_number() {
    let mut n = node();
    let replies = [
        log_reply(&[Entry::Cmd(cmd(1, 1, 1))], 0),
        log_reply(&[Entry::Cmd(cmd(1, 1, 2))], 0),
        log_reply(&[Entry::Cmd(cmd(1, 1, 2))], 0),
    ];
    n.step(&replies, &[], T, &mut rng());
    assert_eq!(
        n.shared_state().sn_get(1),
        Some(1),
        "null commit must unstick the client"
    );
    assert!(
        n.shared_state().untruncated().is_empty(),
        "null executes as a no-op"
    );
}

// --- append requests (step 4, L̄ includes this round's append requests) ---

#[test]
fn append_requests_enter_the_log_with_their_round() {
    let mut n = node();
    let seed_logs: Vec<CompactReply> = n
        .log_entries()
        .map(|log| vec![CompactReply::from_log(ChunkSeq::shared(log.to_vec()), None); 3])
        .unwrap();
    n.step(&seed_logs, &[(cmd(1, 1, 77), 4)], 5, &mut rng());
    let log = n.log_entries().unwrap();
    let found = log
        .iter()
        .find(|t| t.entry == Entry::Cmd(cmd(1, 1, 77)))
        .unwrap();
    assert_eq!(
        found.round, 4,
        "append request keeps the round its sender attached"
    );
}

// --- state recovery (step 4, b_i = 1) ---

#[test]
fn recovering_node_adopts_a_peer_state() {
    let mut n = node();
    n.step(&[], &[], 1, &mut rng()); // → ⊥, b_i = 1
    let peer_state = SharedState::from_entries(
        vec![Entry::Cmd(cmd(1, 1, 77))],
        [(1u32, 1u64)].into_iter().collect(),
    );
    let reply = CompactReply::from_log(
        ChunkSeq::shared(vec![timed(Entry::Cmd(cmd(2, 1, 88)), 2)]),
        Some(peer_state.clone().into()),
    );
    n.step(&[reply.clone(), reply.clone(), reply], &[], 2, &mut rng());
    assert_eq!(n.shared_state().untruncated(), peer_state.untruncated());
    assert_eq!(n.shared_state().sn_get(1), Some(1));
    assert!(
        n.log_entries().is_some(),
        "≥ ℓ replies also restore the log"
    );
}

// --- below-ℓ behavior (step 5) ---

#[test]
fn below_ell_with_one_reply_still_applies_aged_updates() {
    let mut n = node();
    let reply = log_reply(&[Entry::Cmd(cmd(1, 1, 77))], 0);
    n.step(&[reply], &[], T, &mut rng());
    assert_eq!(n.log_entries(), None, "below ℓ the log is ⊥");
    assert_eq!(
        n.shared_state().untruncated(),
        vec![Entry::Cmd(cmd(1, 1, 77))]
    );
    assert_eq!(n.shared_state().sn_get(1), Some(1));
}

#[test]
fn below_ell_with_no_replies_changes_nothing_but_the_log() {
    let mut n = node();
    n.step(&[], &[], 1, &mut rng());
    assert_eq!(n.log_entries(), None);
    assert!(n.shared_state().untruncated().is_empty());
}

#[test]
fn below_ell_recovering_node_adopts_offered_state_before_executing() {
    // Box step 5 (p. 26): with ≥ 1 reply the node "performs the updates ...
    // as described above" — step 4 adopts a peer's S_j (b_i = 1) BEFORE
    // executing the aged prefix. A peer's log holds only its non-committed
    // tail, so executing it onto a stale state would skip earlier commits.
    let mut n = node();
    n.step(&[], &[], 1, &mut rng()); // → ⊥, b_i = 1, empty state
    let peer_state = SharedState::from_entries(
        vec![Entry::Cmd(cmd(1, 1, 10))],
        [(1u32, 1u64)].into_iter().collect(),
    );
    let reply = CompactReply::from_log(
        ChunkSeq::shared(vec![timed(Entry::Cmd(cmd(2, 1, 20)), 2)]),
        Some(peer_state.into()),
    );
    n.step(&[reply], &[], 2 + T, &mut rng());
    assert_eq!(n.log_entries(), None, "below ℓ the log stays ⊥");
    assert_eq!(
        n.shared_state().untruncated(),
        vec![Entry::Cmd(cmd(1, 1, 10)), Entry::Cmd(cmd(2, 1, 20))],
        "peer state adopted first, then the reply's aged prefix on top"
    );
    assert_eq!(n.shared_state().sn_get(1), Some(1));
    assert_eq!(n.shared_state().sn_get(2), Some(1));
}

#[test]
fn below_ell_applies_a_mixed_aged_prefix_and_leaves_the_reply_untouched() {
    // The picked reply's log carries an aged Cmd, an aged Null, then a YOUNG
    // Cmd: the aged prefix executes (Cmd enters state, Null bumps sn only),
    // the young tail does not, and the peer's reply log itself never changes.
    let mut n = node();
    let reply = CompactReply::from_log(
        ChunkSeq::shared(vec![
            timed(Entry::Cmd(cmd(1, 1, 77)), 0),
            timed(Entry::Null { client: 2, sn: 5 }, 0),
            timed(Entry::Cmd(cmd(3, 1, 99)), T),
        ]),
        None,
    );
    let before = reply.l_j.entries().to_vec();
    n.step(std::slice::from_ref(&reply), &[], T, &mut rng());
    assert_eq!(n.log_entries(), None, "below ℓ the log is ⊥");
    assert_eq!(
        n.shared_state().untruncated(),
        vec![Entry::Cmd(cmd(1, 1, 77))],
        "aged Cmd executes; the young tail does not"
    );
    assert_eq!(n.shared_state().sn_get(1), Some(1));
    assert_eq!(n.shared_state().sn_get(2), Some(5), "aged Null bumps sn");
    assert_eq!(n.shared_state().sn_get(3), None, "young Cmd not committed");
    assert_eq!(
        reply.l_j.entries().to_vec(),
        before,
        "the peer's reply log is untouched"
    );
}

#[test]
fn below_ell_state_adoption_uses_the_picked_reply_only() {
    // "picks any one and performs the updates" — one reply drives both the
    // state adoption and the aged-prefix execution; a later reply's state is
    // not scavenged.
    let mut n = node();
    n.step(&[], &[], 1, &mut rng()); // → ⊥, b_i = 1
    let picked = log_reply(&[Entry::Cmd(cmd(2, 1, 20))], 2);
    let unpicked = CompactReply::from_log(
        ChunkSeq::shared(vec![]),
        Some(std::sync::Arc::new(SharedState::from_entries(
            vec![Entry::Cmd(cmd(1, 1, 10))],
            [(1u32, 1u64)].into_iter().collect(),
        ))),
    );
    n.step(&[picked, unpicked], &[], 2 + T, &mut rng());
    assert_eq!(
        n.shared_state().untruncated(),
        vec![Entry::Cmd(cmd(2, 1, 20))],
        "only the picked (first) reply's updates apply"
    );
    assert_eq!(n.shared_state().sn_get(1), None);
}
