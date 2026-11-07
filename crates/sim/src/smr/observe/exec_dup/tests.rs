use super::*;
use crate::smr::observe::safety::PrefixChecker;
use protocol::compact::ClientCommand;
use protocol::shared_state::SharedState;
use std::collections::BTreeMap;

fn cmd(client: u32, sn: u64, op: u64) -> Entry {
    Entry::Cmd(ClientCommand { client, sn, op })
}

fn state(entries: &[Entry]) -> SharedState {
    SharedState::from_entries(entries.to_vec(), BTreeMap::new())
}

/// One round of the engine's wiring: canonical path while the prefix check passes, per-node after.
fn round(
    checker: &mut PrefixChecker,
    oracle: &mut ExecDupOracle,
    prefix_ok: &mut bool,
    states: &[SharedState],
    replaced: &[bool],
    r: usize,
) {
    let views: Vec<ExecutedView<'_>> = states.iter().map(SharedState::executed).collect();
    if *prefix_ok && !checker.check_round(&views, 0) {
        *prefix_ok = false;
    }
    if *prefix_ok {
        let canonical = checker.canonical();
        oracle.observe_canonical(canonical.start(), canonical.retained(), r);
    } else {
        oracle.observe_nodes(&views, replaced, r);
    }
}

#[test]
fn a_repeat_on_every_server_passes_the_prefix_check_but_trips_the_oracle() {
    let mut checker = PrefixChecker::new(3);
    let mut oracle = ExecDupOracle::new(3);
    let mut ok = true;
    let clean = [cmd(1, 1, 10), cmd(2, 1, 20)];
    let dup = [cmd(1, 1, 10), cmd(2, 1, 20), cmd(1, 1, 10)];
    let s1 = vec![state(&clean), state(&clean), state(&clean[..1])];
    round(&mut checker, &mut oracle, &mut ok, &s1, &[false; 3], 1);
    assert_eq!(oracle.tripped_round(), None);
    let s2 = vec![state(&dup), state(&dup), state(&dup)];
    round(&mut checker, &mut oracle, &mut ok, &s2, &[false; 3], 2);
    assert!(
        ok,
        "every server holds the same sequence: prefix consistent"
    );
    assert_eq!(oracle.tripped_round(), Some(2));
}

#[test]
fn a_clean_history_never_trips() {
    let mut checker = PrefixChecker::new(2);
    let mut oracle = ExecDupOracle::new(2);
    let mut ok = true;
    let seq: Vec<Entry> = (1..=6).map(|sn| cmd(7, sn, sn * 100)).collect();
    for r in 1..=6 {
        let s = vec![state(&seq[..r]), state(&seq[..r / 2])];
        round(&mut checker, &mut oracle, &mut ok, &s, &[false; 2], r);
    }
    assert!(ok);
    assert_eq!(oracle.tripped_round(), None);
}

#[test]
fn the_same_slot_with_a_different_op_is_a_double_execution() {
    let mut oracle = ExecDupOracle::new(1);
    oracle.observe_canonical(0, &[cmd(1, 1, 10), cmd(1, 1, 11)], 4);
    assert_eq!(oracle.tripped_round(), Some(4));
}

#[test]
fn the_latch_keeps_its_first_round() {
    let mut oracle = ExecDupOracle::new(1);
    oracle.observe_canonical(0, &[cmd(1, 1, 10), cmd(1, 1, 10)], 3);
    oracle.observe_canonical(
        0,
        &[cmd(1, 1, 10), cmd(1, 1, 10), cmd(2, 1, 1), cmd(2, 1, 1)],
        9,
    );
    assert_eq!(oracle.tripped_round(), Some(3));
}

#[test]
fn after_a_split_a_repeat_on_one_branch_trips() {
    let mut checker = PrefixChecker::new(2);
    let mut oracle = ExecDupOracle::new(2);
    let mut ok = true;
    let (a, b, c) = (cmd(1, 1, 1), cmd(2, 1, 2), cmd(3, 1, 3));
    let start = [
        state(std::slice::from_ref(&a)),
        state(std::slice::from_ref(&a)),
    ];
    round(&mut checker, &mut oracle, &mut ok, &start, &[false; 2], 1);
    let split = [
        state(&[a.clone(), b.clone()]),
        state(&[a.clone(), c.clone()]),
    ];
    round(&mut checker, &mut oracle, &mut ok, &split, &[false; 2], 2);
    assert!(!ok, "the branches diverge");
    assert_eq!(oracle.tripped_round(), None, "no server repeats a slot yet");
    let later = [
        state(&[a.clone(), b.clone(), c.clone()]),
        state(&[a.clone(), c.clone(), b.clone()]),
    ];
    round(&mut checker, &mut oracle, &mut ok, &later, &[false; 2], 3);
    assert_eq!(
        oracle.tripped_round(),
        None,
        "each branch executes b and c once"
    );
    let dup = [
        state(&[a.clone(), b.clone(), c.clone()]),
        state(&[a.clone(), c.clone(), b.clone(), c.clone()]),
    ];
    round(&mut checker, &mut oracle, &mut ok, &dup, &[false; 2], 4);
    assert_eq!(oracle.tripped_round(), Some(4));
}

#[test]
fn a_repeat_of_a_forgotten_entry_is_still_caught() {
    let mut oracle = ExecDupOracle::new(1);
    let (a, b) = (cmd(1, 1, 1), cmd(2, 1, 2));
    oracle.observe_canonical(0, &[a.clone(), b.clone()], 1);
    let mut s = state(&[a.clone(), b.clone(), a.clone()]);
    s.forget_committed_prefix(2);
    oracle.observe_nodes(&[s.executed()], &[false], 2);
    assert_eq!(oracle.tripped_round(), Some(2));
}

#[test]
fn a_replaced_state_is_rescanned_not_appended() {
    let mut oracle = ExecDupOracle::new(1);
    let (a, b, c) = (cmd(1, 1, 1), cmd(2, 1, 2), cmd(3, 1, 3));
    let first = state(&[a.clone(), b.clone()]);
    oracle.observe_nodes(&[first.executed()], &[false], 1);
    // An adopted peer state of the same length: read as an append it would look unchanged.
    let adopted = state(&[a.clone(), c.clone()]);
    oracle.observe_nodes(&[adopted.executed()], &[true], 2);
    let grown = state(&[a.clone(), c.clone(), b.clone()]);
    oracle.observe_nodes(&[grown.executed()], &[false], 3);
    assert_eq!(oracle.tripped_round(), None, "b left with the old state");
    let dup = state(&[a.clone(), c.clone(), b.clone(), c.clone()]);
    oracle.observe_nodes(&[dup.executed()], &[false], 4);
    assert_eq!(oracle.tripped_round(), Some(4));
}

#[test]
fn a_repeat_of_a_slot_released_from_the_canonical_front_trips() {
    let mut oracle = ExecDupOracle::new(1);
    let (a, b, c) = (cmd(1, 1, 1), cmd(2, 1, 2), cmd(3, 1, 3));
    oracle.observe_canonical(0, &[a.clone(), b.clone(), c.clone()], 1);
    oracle.observe_canonical(2, &[c.clone(), a.clone()], 2);
    assert_eq!(oracle.tripped_round(), Some(2));
}

#[test]
fn out_of_order_sequence_numbers_are_members_once_each() {
    let seq = [cmd(1, 2, 2), cmd(1, 1, 1), cmd(1, 4, 4), cmd(1, 3, 3)];
    for repeat in &seq {
        let mut oracle = ExecDupOracle::new(1);
        oracle.observe_canonical(0, &seq, 1);
        oracle.observe_canonical(0, &seq, 2);
        assert_eq!(oracle.tripped_round(), None, "re-reading seen positions");
        let mut longer = seq.to_vec();
        longer.push(repeat.clone());
        oracle.observe_canonical(0, &longer, 3);
        assert_eq!(oracle.tripped_round(), Some(3), "{repeat:?} repeated");
    }
}

#[test]
fn a_slot_released_below_a_node_base_still_counts_against_that_node() {
    let mut oracle = ExecDupOracle::new(1);
    let (a, b) = (cmd(1, 1, 1), cmd(2, 1, 2));
    oracle.observe_canonical(0, &[a.clone(), b.clone()], 1);
    oracle.observe_canonical(2, &[], 2);
    let mut s = state(&[a.clone(), b.clone(), a.clone()]);
    s.forget_committed_prefix(2);
    oracle.observe_nodes(&[s.executed()], &[false], 3);
    assert_eq!(oracle.tripped_round(), Some(3));
}

#[test]
#[should_panic(expected = "below the oracle's release line")]
fn a_node_base_below_the_release_line_fails_loudly() {
    let mut oracle = ExecDupOracle::new(1);
    let (a, b) = (cmd(1, 1, 1), cmd(2, 1, 2));
    oracle.observe_canonical(0, &[a.clone(), b.clone()], 1);
    oracle.observe_canonical(2, &[], 2);
    let mut s = state(&[a.clone(), b.clone()]);
    s.forget_committed_prefix(1);
    oracle.observe_nodes(&[s.executed()], &[false], 3);
}

#[test]
fn released_positions_do_not_stay_resident() {
    let mut oracle = ExecDupOracle::new(1);
    let seq: Vec<Entry> = (1..=2000).map(|sn| cmd(7, sn, sn)).collect();
    oracle.observe_canonical(0, &seq, 1);
    let full = oracle.heap_bytes();
    oracle.observe_canonical(2000, &[], 2);
    assert!(
        oracle.heap_bytes() < full / 10,
        "one client's contiguous run released to the end still holds {} of {full} bytes",
        oracle.heap_bytes()
    );
}

#[test]
fn slot_membership_matches_a_plain_set_under_any_insertion_order() {
    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = |bound: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % bound
    };
    for _ in 0..200 {
        let mut set = SlotSet::default();
        let mut plain = std::collections::HashSet::new();
        for _ in 0..60 {
            let slot = (next(12) as u32, next(4));
            assert_eq!(set.insert(slot), plain.insert(slot), "insert {slot:?}");
            for client in 0..12 {
                for sn in 0..5 {
                    assert_eq!(
                        set.contains((client, sn)),
                        plain.contains(&(client, sn)),
                        "({client}, {sn}) after inserting {slot:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn single_command_clients_committed_in_order_stay_one_run() {
    let mut set = SlotSet::default();
    for client in 0..5000 {
        set.insert((client, 1));
    }
    assert!(set.contains((4999, 1)) && !set.contains((5000, 1)) && !set.contains((7, 2)));
    assert!(
        set.heap_bytes() < 256,
        "5000 contiguous one-command clients cost {} bytes",
        set.heap_bytes()
    );
}
