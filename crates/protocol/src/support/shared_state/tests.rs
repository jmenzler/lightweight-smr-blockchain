use super::*;
use crate::compact::ClientCommand;

fn entry(op: u64) -> Entry {
    Entry::Cmd(ClientCommand {
        client: 1,
        sn: op,
        op,
    })
}

#[test]
fn content_eq_refuses_a_pair_agreeing_only_above_the_forget_line() {
    // Position 16 is retained-but-dead after forgetting to 17.
    let build = |at_16: u64| {
        let mut s = SharedState::default();
        for i in 0..64u64 {
            s.push_executed(entry(if i == 16 { at_16 } else { i }));
        }
        s.forget_committed_prefix(17);
        s
    };
    let a = build(16);
    let b = build(9999);

    assert_eq!(a.executed_offset(), b.executed_offset(), "same frame");
    assert_eq!(a.logical_len(), b.logical_len(), "same length");
    assert!(
        a.executed == b.executed,
        "the permissive comparator accepts them — which is what makes the \
             strictness below load-bearing rather than incidental"
    );
    assert!(
        !a.content_eq(&b),
        "the interning comparator accepted a pair that disagrees on a \
             retained entry, so one allocation would answer for both"
    );
}

#[test]
fn content_eq_refuses_a_pair_that_forgot_different_amounts() {
    let build = |forget: u64| {
        let mut s = SharedState::default();
        for i in 0..64u64 {
            s.push_executed(entry(i));
        }
        s.forget_committed_prefix(forget);
        s
    };
    let a = build(17);
    let b = build(25);

    assert_eq!(a.logical_len(), b.logical_len(), "same history executed");
    assert_ne!(
        a.executed_offset(),
        b.executed_offset(),
        "but different amounts forgotten"
    );
    assert!(
        !a.content_eq(&b),
        "the comparator ignored the forget line — two states retaining \
             different prefixes would have been given one allocation"
    );
}

#[test]
fn forgetting_keeps_what_is_held_inside_the_live_window() {
    let mut s = SharedState::default();
    let mut high_water = 0;
    for i in 0..2000u64 {
        s.push_executed(entry(i));
        s.forget_committed_prefix(i.saturating_sub(50));
        high_water = high_water.max(s.retained_len());
    }
    assert_eq!(s.logical_len(), 2000);
    let live = (s.logical_len() - s.executed_offset()) as usize;
    assert_eq!(live, 51);
    assert!(
        high_water < live + K_STATE,
        "retained {high_water} for a {live}-entry live window — more than \
             one chunk of slack, so the front-drop is re-chunking"
    );
}

#[test]
fn a_compaction_does_not_move_any_logical_index() {
    let mut s = SharedState::default();
    for i in 0..64u64 {
        s.push_executed(entry(i));
    }
    for upto in 1..64u64 {
        s.forget_committed_prefix(upto);
        assert_eq!(s.logical_len(), 64);
        for i in upto..64 {
            assert_eq!(s.executed().get(i), Some(&entry(i)), "index {i}");
        }
    }
}

fn cmd(client: ClientId, sn: u64, op: u64) -> Entry {
    Entry::Cmd(ClientCommand { client, sn, op })
}

fn bot(client: ClientId, sn: u64) -> Entry {
    Entry::Null { client, sn }
}

fn committed(entries: &[Entry], repeated: RepeatedCommit) -> SharedState {
    let mut s = SharedState::default();
    for e in entries {
        s.commit(e, repeated);
    }
    s
}

#[test]
fn skip_drops_a_command_whose_slot_was_committed_before() {
    let (x, y) = (cmd(7, 1, 70), cmd(8, 1, 80));
    let s = committed(&[x.clone(), y.clone(), x.clone()], RepeatedCommit::Skip);
    assert_eq!(s.executed().to_vec(), vec![x, y]);
    assert_eq!(s.sn_get(7), Some(1));
}

#[test]
fn skip_drops_a_repeat_of_an_older_slot_without_lowering_sn() {
    let (x1, x2) = (cmd(7, 1, 70), cmd(7, 2, 71));
    let s = committed(&[x1.clone(), x2.clone(), x1.clone()], RepeatedCommit::Skip);
    assert_eq!(s.executed().to_vec(), vec![x1, x2]);
    assert_eq!(s.sn_get(7), Some(2), "sn(c) never moves backwards");
}

#[test]
fn skip_counts_only_the_entries_it_left_out() {
    let mut s = SharedState::default();
    let x = cmd(7, 1, 70);
    assert!(
        s.commit(&x, RepeatedCommit::Skip),
        "a fresh command executes"
    );
    assert!(!s.commit(&x, RepeatedCommit::Skip), "its repeat is skipped");
    assert!(s.commit(&x, RepeatedCommit::Execute), "Execute never skips");
}

/// The precondition: sn 3 committed before sn 2 makes the guard drop sn 2, never executed.
#[test]
fn skip_drops_a_never_executed_command_once_a_later_sn_committed_first() {
    let x3 = cmd(7, 3, 73);
    let mut s = SharedState::from_entries(vec![x3.clone()], [(7, 3)].into());
    assert!(!s.commit(&cmd(7, 2, 72), RepeatedCommit::Skip));
    assert_eq!(s.executed().to_vec(), vec![x3], "sn 2 is lost");
}

#[test]
#[should_panic(expected = "committed out of sn order")]
fn skip_refuses_a_commit_that_skips_ahead_of_sn_c_plus_one() {
    committed(&[cmd(7, 1, 70), cmd(7, 3, 73)], RepeatedCommit::Skip);
}

#[test]
#[should_panic(expected = "committed out of sn order")]
fn skip_refuses_a_bot_that_skips_ahead_of_sn_c_plus_one() {
    committed(&[bot(7, 2)], RepeatedCommit::Skip);
}

#[test]
fn execute_still_runs_a_gap_as_the_box_prints() {
    let (x1, x3) = (cmd(7, 1, 70), cmd(7, 3, 73));
    let s = committed(&[x1.clone(), x3.clone()], RepeatedCommit::Execute);
    assert_eq!(s.executed().to_vec(), vec![x1, x3]);
}

#[test]
fn skip_treats_bot_as_occupying_its_slot() {
    let s = committed(&[bot(7, 1), cmd(7, 1, 70)], RepeatedCommit::Skip);
    assert!(s.executed().to_vec().is_empty(), "⊥ consumed slot 1");
    assert_eq!(s.sn_get(7), Some(1));

    let s = committed(
        &[cmd(7, 1, 70), cmd(7, 2, 71), bot(7, 1)],
        RepeatedCommit::Skip,
    );
    assert_eq!(
        s.sn_get(7),
        Some(2),
        "a late ⊥ for slot 1 does not reopen slot 2"
    );

    let s = committed(&[cmd(7, 1, 70), bot(7, 2)], RepeatedCommit::Skip);
    assert_eq!(s.sn_get(7), Some(2), "a fresh ⊥ still sets sn(c)");
}

#[test]
fn execute_repeats_as_the_boxes_print() {
    let (x1, x2) = (cmd(7, 1, 70), cmd(7, 2, 71));
    let s = committed(
        &[x1.clone(), x2.clone(), x1.clone()],
        RepeatedCommit::Execute,
    );
    assert_eq!(s.executed().to_vec(), vec![x1.clone(), x2, x1]);
    assert_eq!(s.sn_get(7), Some(1), "the repeat sets sn(c) back");
}

/// Skip reads only (S_i, entry): equal states stay equal, but states that already disagree
/// are not reconciled, since one node skips the X the other executes at a later position.
#[test]
fn skip_preserves_agreement_but_does_not_heal_a_disagreement() {
    let (x, y) = (cmd(7, 1, 70), cmd(8, 1, 80));
    let ahead = committed(&[x.clone(), y.clone(), x.clone()], RepeatedCommit::Skip);
    let behind = committed(&[y.clone(), x.clone()], RepeatedCommit::Skip);
    assert_eq!(ahead.executed().to_vec(), vec![x.clone(), y.clone()]);
    assert_eq!(behind.executed().to_vec(), vec![y, x]);
}

#[test]
fn sn_order_check_names_the_first_skipped_sn() {
    let s = SharedState::default();
    let entries = [cmd(7, 1, 70), cmd(7, 3, 73), cmd(7, 5, 75)];
    assert_eq!(
        s.check_sn_order(entries.iter()),
        Err(OutOfSnOrder {
            client: 7,
            sn: 3,
            committed_sn: 1
        })
    );
}

#[test]
fn sn_order_check_accepts_in_order_commits_repeats_and_bots() {
    let s = SharedState::default();
    let entries = [
        cmd(7, 1, 70),
        cmd(8, 1, 80),
        cmd(7, 2, 71),
        cmd(7, 1, 70),
        bot(8, 2),
        Entry::Nop(4),
    ];
    assert_eq!(s.check_sn_order(entries.iter()), Ok(()));
}

#[test]
fn sn_order_check_starts_from_the_committed_sn() {
    let s = committed(&[cmd(7, 1, 70), cmd(7, 2, 71)], RepeatedCommit::Skip);
    assert_eq!(s.check_sn_order([cmd(7, 3, 73)].iter()), Ok(()));
    assert_eq!(
        s.check_sn_order([bot(7, 4)].iter()),
        Err(OutOfSnOrder {
            client: 7,
            sn: 4,
            committed_sn: 2
        })
    );
}
