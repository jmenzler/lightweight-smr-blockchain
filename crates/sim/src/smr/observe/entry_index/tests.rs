use super::*;
use protocol::compact::ClientCommand;

fn cmd(op: u64) -> Entry {
    Entry::Cmd(ClientCommand {
        client: op as u32,
        sn: 1,
        op,
    })
}

#[test]
fn extend_first_indexes_the_first_occurrence_of_every_entry() {
    let mut idx = EntryIndex::new();
    idx.extend_first(0, &[cmd(1), cmd(2), cmd(1)]);
    assert_eq!(
        (
            idx.get_from(&cmd(1), 0),
            idx.get_from(&cmd(2), 0),
            idx.get_from(&cmd(9), 0)
        ),
        (Some(0), Some(1), None)
    );
}

#[test]
fn extending_boundary_by_boundary_answers_like_a_fresh_build() {
    let boundaries = [
        vec![cmd(1), cmd(2)],
        vec![cmd(1), cmd(2), cmd(3), cmd(1)],
        vec![cmd(1), cmd(2), cmd(3), cmd(1), cmd(4)],
    ];
    let mut extended = EntryIndex::new();
    for seq in &boundaries {
        extended.extend_first(0, seq);
        let mut fresh = EntryIndex::new();
        fresh.extend_first(0, seq);
        for op in 1..=5 {
            assert_eq!(
                extended.get_from(&cmd(op), 0),
                fresh.get_from(&cmd(op), 0),
                "op {op}"
            );
        }
    }
}

#[test]
fn a_released_front_does_not_shift_the_positions_behind_it() {
    let whole = [cmd(1), cmd(2), cmd(3), cmd(4), cmd(5)];
    let mut walked = EntryIndex::new();
    walked.extend_first(0, &whole);

    let mut released = EntryIndex::new();
    released.extend_first(0, &whole[..2]);
    // Positions 2.. arrive with the front already dropped.
    released.extend_first(2, &whole[2..]);
    for op in 1..=5 {
        assert_eq!(
            released.get_from(&cmd(op), 0),
            walked.get_from(&cmd(op), 0),
            "op {op}"
        );
    }
    assert_eq!(released.get_from(&cmd(5), 0), Some(4));
}

#[test]
fn positions_at_or_above_the_release_line_survive_it() {
    let mut idx = EntryIndex::new();
    idx.extend_first(0, &[cmd(1), cmd(2), cmd(3), cmd(4), cmd(5)]);
    idx.release_to(2);
    assert_eq!(idx.get_from(&cmd(3), 2), Some(2));
    assert_eq!(idx.get_from(&cmd(5), 3), Some(4));
    assert_eq!(idx.get_from(&cmd(9), 2), None);
}

#[test]
#[should_panic(expected = "below the index's release line")]
fn a_query_floor_below_the_release_line_fails_loudly() {
    let mut idx = EntryIndex::new();
    idx.extend_first(0, &[cmd(1), cmd(2), cmd(3)]);
    idx.release_to(2);
    idx.get_from(&cmd(3), 1);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "occurs below the query floor")]
fn a_query_for_an_entry_below_its_floor_fails_in_tests() {
    let mut idx = EntryIndex::new();
    idx.extend_first(0, &[cmd(1), cmd(2), cmd(3)]);
    idx.release_to(2);
    idx.get_from(&cmd(1), 2);
}

#[test]
fn released_positions_do_not_stay_resident() {
    let mut idx = EntryIndex::new();
    let seq: Vec<Entry> = (1..=2000).map(cmd).collect();
    idx.extend_first(0, &seq);
    let full = idx.heap_bytes();
    idx.release_to(2000);
    assert!(
        idx.heap_bytes() < full / 10,
        "released to the end, the index still holds {} of {full} bytes",
        idx.heap_bytes()
    );
}
