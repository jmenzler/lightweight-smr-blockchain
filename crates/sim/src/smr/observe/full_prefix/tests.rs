use super::*;
use crate::smr::Injection;
use crate::smr::tracker::trackers_for;
use protocol::chunked::CHUNK;
use protocol::compact::ClientCommand;

fn cmd(client: u32, sn: u64) -> Entry {
    Entry::Cmd(ClientCommand {
        client,
        sn,
        op: u64::from(client) * 100 + sn,
    })
}

fn log(entries: &[(Entry, u64)]) -> ChunkSeq<Timed> {
    ChunkSeq::from_vec(
        entries
            .iter()
            .map(|(entry, round)| Timed {
                entry: entry.clone(),
                round: *round,
            })
            .collect(),
    )
}

fn canonical(entries: &[Entry]) -> CanonicalOrder {
    let mut c = CanonicalOrder::new();
    c.extend_from_slice(entries);
    c
}

fn seq(executed: u64, log: &ChunkSeq<Timed>) -> FullSeq<'_> {
    FullSeq { executed, log }
}

#[test]
fn no_server_has_no_common_prefix() {
    assert_eq!(common_prefix(&canonical(&[]), &[]), None);
}

#[test]
fn one_server_shares_its_whole_sequence() {
    let l = log(&[(cmd(1, 1), 1), (cmd(2, 1), 2)]);
    let c = canonical(&[cmd(9, 1)]);
    let got = common_prefix(&c, &[seq(1, &l)]).expect("one server");
    assert_eq!((got.len, got.reference), (3, 0));
}

#[test]
fn equal_executed_parts_compare_the_logs_entry_by_entry() {
    let c = canonical(&[cmd(9, 1), cmd(9, 2)]);
    let a = log(&[(cmd(1, 1), 1), (cmd(2, 1), 2), (cmd(3, 1), 3)]);
    let b = log(&[(cmd(1, 1), 1), (cmd(2, 1), 2), (cmd(4, 1), 3)]);
    assert_eq!(common_prefix(&c, &[seq(2, &a), seq(2, &b)]).unwrap().len, 4);
}

#[test]
fn stamps_are_not_compared() {
    let c = canonical(&[]);
    let a = log(&[(cmd(1, 1), 1), (cmd(2, 1), 2)]);
    let b = log(&[(cmd(1, 1), 7), (cmd(2, 1), 9)]);
    assert_eq!(common_prefix(&c, &[seq(0, &a), seq(0, &b)]).unwrap().len, 2);
}

#[test]
fn a_shorter_executed_part_is_compared_against_the_canonical_order() {
    let c = canonical(&[cmd(9, 1), cmd(9, 2), cmd(9, 3), cmd(9, 4)]);
    let ahead = log(&[(cmd(1, 1), 5)]);
    let behind = log(&[(cmd(9, 3), 3), (cmd(9, 4), 4), (cmd(1, 1), 5)]);
    let got = common_prefix(&c, &[seq(2, &behind), seq(4, &ahead)]).unwrap();
    assert_eq!(
        (got.len, got.reference),
        (5, 1),
        "the reference is the longest executed part"
    );
    let forked = log(&[(cmd(9, 3), 3), (cmd(7, 1), 4), (cmd(1, 1), 5)]);
    assert_eq!(
        common_prefix(&c, &[seq(2, &forked), seq(4, &ahead)])
            .unwrap()
            .len,
        3
    );
    let short = log(&[(cmd(9, 3), 3)]);
    assert_eq!(
        common_prefix(&c, &[seq(4, &ahead), seq(2, &short)])
            .unwrap()
            .len,
        3,
        "a full sequence that ends bounds the prefix"
    );
}

#[test]
fn shared_chunks_are_skipped_but_the_running_minimum_still_binds() {
    let entries: Vec<(Entry, u64)> = (0..3 * CHUNK as u64 + 2)
        .map(|i| (cmd(1, i + 1), i))
        .collect();
    let a = log(&entries);
    let b = a.clone();
    let c = canonical(&[]);
    assert_eq!(
        common_prefix(&c, &[seq(0, &a), seq(0, &b)]).unwrap().len,
        entries.len() as u64
    );
    let short = log(&entries[..CHUNK + 3]);
    assert_eq!(
        common_prefix(&c, &[seq(0, &a), seq(0, &short), seq(0, &b)])
            .unwrap()
            .len,
        CHUNK as u64 + 3
    );
    let mut diverged = entries.clone();
    diverged[2 * CHUNK + 1].0 = cmd(5, 1);
    let d = log(&diverged);
    assert_eq!(
        common_prefix(&c, &[seq(0, &a), seq(0, &b), seq(0, &d)])
            .unwrap()
            .len,
        2 * CHUNK as u64 + 1
    );
}

#[test]
fn unsettled_age_is_the_oldest_stamp_at_or_past_the_common_prefix() {
    let a = log(&[(cmd(1, 1), 10), (cmd(2, 1), 30), (cmd(3, 1), 20)]);
    let b = log(&[(cmd(1, 1), 10), (cmd(4, 1), 25)]);
    let seqs = [seq(0, &a), seq(0, &b)];
    assert_eq!(
        oldest_unsettled_age(40, 1, &seqs),
        20,
        "r40 − stamp 20; the settled stamp 10 is ignored"
    );
    assert_eq!(
        oldest_unsettled_age(40, 3, &[seq(0, &a)]),
        0,
        "nothing past the prefix"
    );
    let behind = log(&[(cmd(9, 3), 1)]);
    assert_eq!(
        oldest_unsettled_age(40, 2, &[seq(4, &behind)]),
        39,
        "an executed part past the prefix leaves every log entry unsettled"
    );
}

// The contested command x sits where the two logs disagree, so it is not fixed until they agree.
#[test]
fn a_contested_command_is_fixed_only_once_every_log_agrees_through_it() {
    let injections = [
        Injection {
            round: 1,
            client: 1,
            op: 101,
            target: None,
        },
        Injection {
            round: 1,
            client: 2,
            op: 201,
            target: None,
        },
        Injection {
            round: 1,
            client: 3,
            op: 301,
            target: None,
        },
    ];
    let (mut trackers, _) = trackers_for(&injections);
    let (a, x, y) = (cmd(1, 1), cmd(2, 1), cmd(3, 1));
    let c = canonical(&[]);
    let mut index = EntryIndex::new();
    index.extend_first(0, c.retained());

    let one = log(&[(a.clone(), 1), (x.clone(), 2), (y.clone(), 2)]);
    let two = log(&[(a.clone(), 1), (y.clone(), 2), (x.clone(), 2)]);
    let age = observe_full_prefix(
        3,
        &c,
        &index,
        0,
        &[seq(0, &one), seq(0, &two)],
        &mut trackers,
    );
    assert_eq!(
        age,
        Some(UnsettledAge {
            full: 1,
            contested: 1
        }),
        "x and y both stamped r2 sit past the common prefix"
    );
    let fixed: Vec<_> = trackers.iter().map(|t| t.full_prefix_fixed_round).collect();
    assert_eq!(
        fixed,
        vec![Some(3), None, None],
        "only the agreed a is fixed"
    );

    let agreed = log(&[(a.clone(), 1), (x.clone(), 2), (y.clone(), 2)]);
    observe_full_prefix(
        4,
        &c,
        &index,
        0,
        &[seq(0, &one), seq(0, &agreed)],
        &mut trackers,
    );
    let fixed: Vec<_> = trackers.iter().map(|t| t.full_prefix_fixed_round).collect();
    assert_eq!(
        fixed,
        vec![Some(3), Some(4), Some(4)],
        "set once, at the first agreeing round"
    );
}

#[test]
fn an_executed_command_is_found_through_the_canonical_index() {
    let injections = [Injection {
        round: 1,
        client: 1,
        op: 101,
        target: None,
    }];
    let (mut trackers, _) = trackers_for(&injections);
    let c = canonical(&[cmd(1, 1), cmd(9, 1)]);
    let mut index = EntryIndex::new();
    index.extend_first(0, c.retained());
    let tail = log(&[(cmd(4, 1), 5)]);
    let lagging = log(&[(cmd(7, 1), 5)]);
    observe_full_prefix(
        6,
        &c,
        &index,
        0,
        &[seq(2, &tail), seq(1, &lagging)],
        &mut trackers,
    );
    assert_eq!(
        trackers[0].full_prefix_fixed_round,
        Some(6),
        "position 0 lies inside the common prefix 1"
    );
}

// A lagging server holds a strict prefix of the reference and contradicts nobody; a
// disagreeing one holds a different entry. The full prefix stops at both, the contested at the second.
#[test]
fn the_contested_prefix_ignores_a_lagging_server_but_not_a_disagreeing_one() {
    let (a, b, c3, d, x) = (cmd(1, 1), cmd(2, 1), cmd(3, 1), cmd(4, 1), cmd(5, 1));
    let reference = log(&[
        (a.clone(), 1),
        (b.clone(), 2),
        (c3.clone(), 3),
        (d.clone(), 4),
    ]);
    let lagging = log(&[(a.clone(), 1), (b.clone(), 2)]);
    let disagreeing = log(&[
        (a.clone(), 1),
        (b.clone(), 2),
        (c3.clone(), 3),
        (x.clone(), 5),
    ]);
    let canon = canonical(&[]);
    let seqs = [seq(0, &lagging), seq(0, &reference), seq(0, &disagreeing)];
    let got = common_prefix(&canon, &seqs).unwrap();
    assert_eq!((got.len, got.contested), (2, 3));
    assert_eq!(
        got.reference, 1,
        "among equal executed parts the longest sequence is the reference"
    );
    let (mut trackers, _) = trackers_for(&[]);
    let age = observe_full_prefix(10, &canon, &EntryIndex::new(), 0, &seqs, &mut trackers);
    assert_eq!(
        age,
        Some(UnsettledAge {
            full: 7,
            contested: 6
        }),
        "full: c (r3) at position 2 counts; contested: only d (r4) and x (r5) from position 3"
    );
    let alone = common_prefix(&canon, &[seq(0, &lagging), seq(0, &reference)]).unwrap();
    assert_eq!(
        (alone.len, alone.contested),
        (2, 4),
        "with nobody disagreeing the contested prefix is the whole reference"
    );
}
