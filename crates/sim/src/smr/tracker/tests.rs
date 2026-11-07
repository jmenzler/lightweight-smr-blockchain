use super::*;

fn inj(round: usize, client: u32, op: u64) -> Injection {
    Injection {
        round,
        client,
        op,
        target: None,
    }
}

#[test]
fn the_inert_prefix_stops_at_the_first_live_tracker() {
    let ts = trackers_for(&[inj(1, 1, 10), inj(1, 2, 20), inj(1, 3, 30), inj(1, 4, 40)]).0;
    // Inert by op: 10 and 20 lead, 30 is live, 40 is inert but behind it.
    let inert = |t: &CommandTracker| t.op != 30;
    assert_eq!(
        advance_inert_prefix(&ts, 0, inert),
        2,
        "a live tracker stops the advance"
    );
}

#[test]
fn the_inert_prefix_only_moves_forward() {
    let ts = trackers_for(&[inj(1, 1, 10), inj(1, 2, 20), inj(1, 3, 30)]).0;
    assert_eq!(advance_inert_prefix(&ts, 2, |_| false), 2);
    assert_eq!(advance_inert_prefix(&ts, 2, |_| true), 3);
    assert_eq!(
        advance_inert_prefix(&ts, 3, |_| true),
        3,
        "cannot pass the end"
    );
}

fn tracked() -> CommandTracker {
    trackers_for(&[inj(1, 1, 7)]).0.remove(0)
}

fn observe_logs(t: &mut CommandTracker, round: usize, logs: &[Option<Vec<Command>>]) {
    let post: Vec<Option<&[Command]>> = logs.iter().map(|l| l.as_deref()).collect();
    t.observe(
        round,
        &[],
        &vec![true; logs.len()],
        logs.len() as u32,
        &post,
        false,
    );
}

#[test]
fn coverage_latches_once_every_non_bot_log_holds_the_command() {
    let mut t = tracked();
    observe_logs(&mut t, 1, &[Some(vec![0, 7]), Some(vec![0]), None]);
    assert_eq!(t.all_logs_round, None, "one log still lacks it");
    observe_logs(&mut t, 2, &[Some(vec![0, 7]), Some(vec![0, 7]), None]);
    assert_eq!((t.all_logs_round, t.prefix_fixed_round), (Some(2), Some(2)));
}

#[test]
fn the_prefix_is_not_fixed_while_logs_place_the_command_differently() {
    let mut t = tracked();
    observe_logs(&mut t, 1, &[Some(vec![0, 7]), Some(vec![0, 9, 7])]);
    assert_eq!((t.all_logs_round, t.prefix_fixed_round), (Some(1), None));
}

#[test]
#[should_panic(expected = "left a log")]
fn a_command_leaving_a_log_after_full_coverage_is_a_driver_bug() {
    let mut t = tracked();
    observe_logs(&mut t, 1, &[Some(vec![0, 7]), Some(vec![0, 7])]);
    observe_logs(&mut t, 2, &[Some(vec![0, 7]), Some(vec![0])]);
}

#[test]
#[should_panic(expected = "prefix un-fixed")]
fn a_fixed_prefix_that_changes_is_a_driver_bug() {
    let mut t = tracked();
    observe_logs(&mut t, 1, &[Some(vec![0, 7]), Some(vec![0, 7])]);
    observe_logs(&mut t, 2, &[Some(vec![0, 8, 7]), Some(vec![0, 8, 7])]);
}

// Def 1.3, p. 6: same index is agreement; Lemma 3.8's identical prefix is a stronger landmark.
#[test]
fn index_agreement_latches_where_the_prefix_does_not() {
    let mut t = tracked();
    observe_logs(&mut t, 2, &[Some(vec![0, 1, 7]), Some(vec![0, 2, 7])]);
    observe_logs(&mut t, 3, &[Some(vec![0, 1, 7]), Some(vec![0, 2, 7])]);
    assert_eq!(t.index_agreed_round, Some(2));
    assert_eq!(t.prefix_fixed_round, None);
    observe_logs(&mut t, 4, &[Some(vec![0, 1, 2, 7]), Some(vec![0, 1, 2, 7])]);
    assert_eq!(
        t.index_agreed_round,
        Some(4),
        "the index moved, so agreement restarts"
    );
    assert_eq!(t.prefix_fixed_round, Some(4));
}

#[test]
fn a_broken_index_agreement_clears_the_landmark() {
    let mut t = tracked();
    observe_logs(&mut t, 2, &[Some(vec![0, 1, 7]), Some(vec![0, 2, 7])]);
    observe_logs(&mut t, 3, &[Some(vec![0, 1, 7]), Some(vec![0, 7, 2])]);
    assert_eq!(t.index_agreed_round, None);
}

#[test]
fn the_agreed_index_is_each_logs_first_occurrence() {
    let mut t = tracked();
    observe_logs(&mut t, 1, &[Some(vec![0, 7]), Some(vec![7, 7])]);
    assert_eq!(
        t.agreed_index, None,
        "log 1 holds 7 at index 1 but first at 0"
    );
    observe_logs(&mut t, 2, &[Some(vec![0, 7]), Some(vec![0, 7, 7])]);
    assert_eq!((t.agreed_index, t.index_agreed_round), (Some(1), Some(2)));
}

#[test]
fn a_log_without_the_command_leaves_no_agreed_index() {
    let mut t = tracked();
    observe_logs(
        &mut t,
        1,
        &[Some(vec![0, 7]), Some(vec![0, 9]), Some(vec![0, 7])],
    );
    assert_eq!((t.all_logs_round, t.agreed_index), (None, None));
    observe_logs(&mut t, 2, &[Some(vec![9, 0]), Some(vec![0, 7])]);
    assert_eq!(
        (t.all_logs_round, t.agreed_index),
        (None, None),
        "the first log lacks it"
    );
}
