use super::*;
use protocol::compact::ClientCommand;

fn cmd(op: u64) -> Entry {
    Entry::Cmd(ClientCommand {
        client: op as u32,
        sn: 1,
        op,
    })
}

fn state(entries: &[Entry], forget: u64) -> SharedState {
    let mut s = SharedState::from_entries(entries.to_vec(), Default::default());
    s.forget_committed_prefix(forget);
    s
}

#[test]
fn a_forgotten_prefix_does_not_move_the_chain_value() {
    let entries: Vec<Entry> = (1..=6).map(cmd).collect();
    let canon = CanonicalOrder::from_entries(&entries);
    let whole = state_digest(&canon, &state(&entries, 0));
    for forget in 0..=entries.len() as u64 {
        assert_eq!(
            state_digest(&canon, &state(&entries, forget)),
            whole,
            "forgetting {forget} moved the chain value"
        );
    }
}

#[test]
fn a_divergence_above_the_offset_moves_the_chain_value() {
    let entries: Vec<Entry> = (1..=6).map(cmd).collect();
    let canon = CanonicalOrder::from_entries(&entries);
    let mut forked = entries.clone();
    forked[4] = cmd(99);
    for forget in 0..=4 {
        assert_ne!(
            state_digest(&canon, &state(&entries, forget)),
            state_digest(&canon, &state(&forked, forget)),
            "forget {forget}: a fork above the offset hashed the same"
        );
    }
}

#[test]
fn the_same_entries_at_different_positions_differ() {
    let a = CanonicalOrder::from_entries(&[cmd(1), cmd(2)]);
    let b = CanonicalOrder::from_entries(&[cmd(9), cmd(1), cmd(2)]);
    assert_ne!(a.digest_at(2), b.digest_at(3));
}

#[test]
fn releasing_keeps_absolute_positions_and_digests() {
    let entries: Vec<Entry> = (1..=6).map(cmd).collect();
    let full = CanonicalOrder::from_entries(&entries);
    let mut cut = CanonicalOrder::from_entries(&entries);
    cut.release_to(4);
    assert_eq!(cut.start(), 4);
    assert_eq!(cut.len(), full.len());
    for at in 4..=6 {
        assert_eq!(cut.digest_at(at), full.digest_at(at), "at {at}");
    }
    assert_eq!(cut.slice(4, 6), full.slice(4, 6));
}

#[test]
#[should_panic(expected = "below the released prefix")]
fn reading_a_released_digest_panics_rather_than_answering() {
    let mut canon = CanonicalOrder::from_entries(&[cmd(1), cmd(2), cmd(3)]);
    canon.release_to(2);
    canon.digest_at(1);
}

#[test]
fn a_position_below_the_offset_resolves_through_the_canonical_order() {
    let entries: Vec<Entry> = (1..=5).map(cmd).collect();
    let canon = CanonicalOrder::from_entries(&entries);
    let s = state(&entries, 3);
    let ex = Executed {
        view: s.executed(),
        canonical: &canon,
    };
    for op in 1..=5 {
        assert_eq!(
            ex.front_scan(&cmd(op)),
            Some(op as u32 - 1),
            "op {op} was not found across the offset"
        );
    }
    assert_eq!(ex.front_scan(&cmd(9)), None);
}

#[test]
fn retained_front_scan_prefers_an_earlier_duplicate_to_the_live_tail() {
    let entries = [cmd(1), cmd(7), cmd(2), cmd(7), cmd(3)];
    let mut canon = CanonicalOrder::from_entries(&entries);
    canon.release_to(1);
    let state = state(&entries, 3);
    let ex = Executed {
        view: state.executed(),
        canonical: &canon,
    };

    assert_eq!(canon.start(), 1);
    assert_eq!(ex.front_scan(&cmd(7)), Some(1));
}

#[test]
fn holds_agrees_with_the_front_scan_across_the_offset_and_a_released_prefix() {
    let entries: Vec<Entry> = (1..=150).map(cmd).collect();
    for (release, forget) in [(0, 0), (0, 70), (40, 70), (70, 70)] {
        let mut canon = CanonicalOrder::from_entries(&entries[..100]);
        canon.release_to(release);
        let s = state(&entries, forget);
        let ex = Executed {
            view: s.executed(),
            canonical: &canon,
        };
        // Released positions are never scanned for; op k sits at position k - 1.
        for op in std::iter::once(0).chain(release + 1..=151) {
            assert_eq!(
                ex.holds(&cmd(op)),
                ex.front_scan(&cmd(op)).is_some(),
                "op {op}, released to {release}, forgot to {forget}"
            );
        }
    }
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "answers differently from the full one")]
fn a_scan_for_a_released_entry_fails_in_tests() {
    let entries: Vec<Entry> = (1..=6).map(cmd).collect();
    let mut canon = CanonicalOrder::from_entries(&entries);
    canon.release_to(3);
    let s = state(&entries, 4);
    Executed {
        view: s.executed(),
        canonical: &canon,
    }
    .holds(&cmd(1));
}

fn checkpoint(entries: &[Entry], forget: u64, w: u64) -> Checkpoint {
    Checkpoint {
        s: std::sync::Arc::new(state(entries, forget)),
        p: Some(vec![protocol::compact::Timed {
            entry: cmd(9),
            round: w,
        }]),
        w,
        certs: None,
    }
}

#[test]
fn the_digest_cache_answers_as_the_uncached_digests_do() {
    let entries: Vec<Entry> = (1..=6).map(cmd).collect();
    let canon = CanonicalOrder::from_entries(&entries);
    let mut forked = entries.clone();
    forked[4] = cmd(99);
    let checkpoints = [
        checkpoint(&entries, 0, 1),
        checkpoint(&entries, 3, 1),
        checkpoint(&forked, 2, 1),
        checkpoint(&entries[..4], 4, 2),
    ];
    let mut cache = DigestCache::new(&canon);
    for cp in &checkpoints {
        assert_eq!(cache.signature(cp), ForkSignature::of(&canon, cp));
        assert_eq!(cache.state(&cp.s), state_digest(&canon, &cp.s));
    }
}

#[test]
fn the_digest_cache_computes_each_allocation_once() {
    let entries: Vec<Entry> = (1..=6).map(cmd).collect();
    let canon = CanonicalOrder::from_entries(&entries);
    let shared = std::sync::Arc::new(checkpoint(&entries, 2, 1));
    let holders = [
        std::sync::Arc::clone(&shared),
        std::sync::Arc::clone(&shared),
    ];
    let equal_copy = checkpoint(&entries, 2, 1);
    let mut cache = DigestCache::new(&canon);
    for cp in &holders {
        cache.signature(cp);
        cache.state(&cp.s);
    }
    assert_eq!(
        cache.computed(),
        (1, 1),
        "one allocation, one state digest and one signature"
    );
    cache.signature(&equal_copy);
    assert_eq!(
        cache.computed(),
        (2, 2),
        "an equal checkpoint in another allocation is computed anew"
    );
}
