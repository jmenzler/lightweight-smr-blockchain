use super::super::canonical::{CanonicalOrder, Executed};
use super::super::entry_index::EntryIndex;
use super::*;
use protocol::compact::{ClientCommand, Entry, SharedState};

fn cmd(op: u64) -> Entry {
    Entry::Cmd(ClientCommand {
        client: op as u32,
        sn: 1,
        op,
    })
}

fn state(ex: &[Entry], frontier: u64) -> SharedState {
    let mut s = SharedState::from_entries(ex.to_vec(), Default::default());
    s.forget_committed_prefix(frontier);
    s
}

#[test]
fn the_floor_is_fixed_at_open() {
    assert_eq!(CanonCursor::new(2).floor(), 2);
}

fn canon_replay(canonical: &[Entry], entry: &Entry) {
    let order = CanonicalOrder::from_entries(canonical);
    let mut idx = EntryIndex::new();
    for cover in 0..=canonical.len() {
        idx.extend_first(0, &canonical[..cover]);
        for len in 0..=cover {
            for frontier in 0..=len {
                let s = state(&canonical[..len], frontier as u64);
                let seen = Executed {
                    view: s.executed(),
                    canonical: &order,
                };
                assert_eq!(
                    canon_lookup(idx.get_from(entry, 0), s.executed()),
                    seen.front_scan(entry),
                    "cover {cover} len {len} frontier {frontier}"
                );
            }
        }
    }
}

#[test]
fn a_canon_hit_appears_exactly_when_the_prefix_reaches_it() {
    canon_replay(&[cmd(1), cmd(2), cmd(3), cmd(4)], &cmd(3));
}

#[test]
fn an_entry_outside_the_canonical_order_stays_none() {
    canon_replay(&[cmd(1), cmd(2), cmd(3)], &cmd(9));
}

#[test]
fn a_duplicated_entry_resolves_to_its_first_occurrence() {
    canon_replay(&[cmd(1), cmd(5), cmd(2), cmd(5)], &cmd(5));
}

#[test]
fn a_hit_below_a_risen_offset_still_resolves() {
    let ex = [cmd(5), cmd(1), cmd(2)];
    let mut idx = EntryIndex::new();
    idx.extend_first(0, &ex);
    assert_eq!(
        canon_lookup(idx.get_from(&cmd(5), 0), state(&ex, 2).executed()),
        Some(0)
    );
}
