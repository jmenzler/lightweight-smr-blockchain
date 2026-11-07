use super::*;
use crate::smr::observe::safety::PrefixChecker;
use protocol::compact::ClientCommand;
use protocol::shared_state::ExecSeq;

fn cmd(op: u64) -> Entry {
    Entry::Cmd(ClientCommand {
        client: op as u32,
        sn: 1,
        op,
    })
}

fn state(ops: &[u64]) -> ExecSeq {
    ExecSeq::from_vec(ops.iter().copied().map(cmd).collect())
}

/// One observation the way the engines wire it: the no-fork check first, then the oracle.
fn observe(
    checker: &mut PrefixChecker,
    oracle: &mut CommittedOracle,
    states: &[ExecSeq],
    r: usize,
) {
    let views: Vec<ExecutedView<'_>> = states.iter().map(ExecutedView::of).collect();
    let frontier = views.iter().map(ExecutedView::offset).max().unwrap_or(0);
    checker.check_round(&views, frontier);
    oracle.observe(&views, checker.canonical(), r);
}

#[test]
fn a_shrink_is_a_retraction_the_no_fork_check_lets_pass() {
    let mut checker = PrefixChecker::new(1);
    let mut oracle = CommittedOracle::new(1);
    observe(&mut checker, &mut oracle, &[state(&[1, 2])], 1);
    let views = [state(&[1])];
    let ok = checker.check_round(&[ExecutedView::of(&views[0])], 0);
    oracle.observe(&[ExecutedView::of(&views[0])], checker.canonical(), 2);
    assert!(ok, "[a, b] -> [a] is still a prefix of the order");
    assert_eq!(oracle.retractions(), (1, 0));
    assert_eq!(oracle.first_retraction_round(), Some(2));
}

#[test]
fn a_same_length_rewrite_is_a_retraction() {
    let mut checker = PrefixChecker::new(1);
    let mut oracle = CommittedOracle::new(1);
    observe(&mut checker, &mut oracle, &[state(&[1, 2])], 1);
    observe(&mut checker, &mut oracle, &[state(&[1, 3])], 2);
    assert_eq!(oracle.retractions(), (1, 1));
    assert_eq!(oracle.first_retraction_round(), Some(2));
}

#[test]
fn a_longer_rewrite_past_a_sealed_chunk_is_a_retraction() {
    let mut checker = PrefixChecker::new(1);
    let mut oracle = CommittedOracle::new(1);
    let old: Vec<u64> = (1..=20).collect();
    let mut new = old.clone();
    new[3] = 99;
    new.extend(21..=30);
    observe(&mut checker, &mut oracle, &[state(&old)], 1);
    observe(&mut checker, &mut oracle, &[state(&new)], 2);
    assert_eq!(oracle.retractions(), (1, 1));
}

#[test]
fn extensions_by_append_and_by_adopting_an_equal_copy_are_not_retractions() {
    let mut checker = PrefixChecker::new(2);
    let mut oracle = CommittedOracle::new(2);
    let base: Vec<u64> = (1..=20).collect();
    let mut s0 = state(&base);
    let s1 = state(&base[..10]);
    observe(&mut checker, &mut oracle, &[s0.clone(), s1], 1);
    s0.extend((21..=40).map(cmd));
    // Server 1 adopts a separately built, equal-content sequence: no shared allocation.
    let adopted = state(&(1..=40).collect::<Vec<_>>());
    observe(&mut checker, &mut oracle, &[s0, adopted], 2);
    assert_eq!(oracle.retractions(), (0, 0));
    assert_eq!(oracle.first_retraction_round(), None);
}

#[test]
fn strong_safety_flags_a_lagging_useful_server_only_among_the_useful() {
    let mut checker = PrefixChecker::new(3);
    let mut oracle = CommittedOracle::new(3);
    observe(
        &mut checker,
        &mut oracle,
        &[state(&[1, 2]), state(&[1, 2]), state(&[1])],
        1,
    );
    oracle.check_strong_safety(&[true, true, false], 1, false);
    assert_eq!(oracle.strong_safety(), (0, None));
    oracle.check_strong_safety(&[true, false, true], 1, false);
    assert_eq!(oracle.strong_safety(), (1, Some(1)));
}

#[test]
fn strong_safety_compares_content_not_only_length() {
    let mut checker = PrefixChecker::new(2);
    let mut oracle = CommittedOracle::new(2);
    observe(
        &mut checker,
        &mut oracle,
        &[state(&[1, 2]), state(&[1, 3])],
        1,
    );
    oracle.check_strong_safety(&[true, true], 1, false);
    assert_eq!(oracle.strong_safety(), (1, Some(1)));
}

#[test]
fn forgetting_a_committed_prefix_is_not_a_retraction() {
    let mut checker = PrefixChecker::new(1);
    let mut oracle = CommittedOracle::new(1);
    let mut s = state(&(1..=30).collect::<Vec<_>>());
    observe(&mut checker, &mut oracle, std::slice::from_ref(&s), 1);
    s.drop_below(17);
    s.extend((31..=35).map(cmd));
    let views = [ExecutedView::of(&s)];
    assert!(checker.check_round(&views, 17));
    oracle.observe(&views, checker.canonical(), 2);
    assert_eq!(oracle.retractions(), (0, 0));
}

#[test]
fn equal_sequences_with_different_forget_lines_are_equal_for_strong_safety() {
    let mut checker = PrefixChecker::new(2);
    let mut oracle = CommittedOracle::new(2);
    let full = state(&(1..=30).collect::<Vec<_>>());
    let mut forgot = full.clone();
    forgot.drop_below(20);
    observe(&mut checker, &mut oracle, &[full, forgot], 1);
    oracle.check_strong_safety(&[true, true], 1, false);
    assert_eq!(oracle.strong_safety(), (0, None));
}

#[test]
fn a_window_end_disagreement_counts_in_both_tallies_and_no_useful_server_is_no_verdict() {
    let mut checker = PrefixChecker::new(2);
    let mut oracle = CommittedOracle::new(2);
    observe(&mut checker, &mut oracle, &[state(&[1, 2]), state(&[1])], 1);
    assert_eq!(oracle.check_strong_safety(&[false, false], 1, true), None);
    assert_eq!(
        oracle.check_strong_safety(&[true, true], 1, false),
        Some(false)
    );
    assert_eq!(oracle.strong_safety_at_boundaries(), (0, None));
    assert_eq!(
        oracle.check_strong_safety(&[true, true], 2, true),
        Some(false)
    );
    assert_eq!(oracle.strong_safety(), (2, Some(1)));
    assert_eq!(oracle.strong_safety_at_boundaries(), (1, Some(2)));
}

#[test]
fn useful_at_round_end_needs_a_log_and_no_block() {
    let useful = useful_at_round_end(
        [true, true, false, false].into_iter(),
        &[false, true, false, true],
    );
    assert_eq!(useful, vec![true, false, false, false]);
}

#[test]
fn a_refreshed_observation_reuses_its_tail_buffer() {
    let longer = state(&[1, 2, 3]);
    let shorter = state(&[1, 2]);
    let mut seen = Seen::genesis();
    seen.refresh(ExecutedView::of(&longer), 7);
    let buffer = seen.tail.as_ptr();
    seen.refresh(ExecutedView::of(&shorter), 8);
    assert_eq!(seen.tail, vec![cmd(1), cmd(2)]);
    assert_eq!((seen.len, seen.chain), (2, 8));
    assert_eq!(seen.tail.as_ptr(), buffer, "the tail was reallocated");
}
