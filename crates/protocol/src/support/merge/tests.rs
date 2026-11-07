use super::*;
use crate::chunked::CHUNK;
use crate::compact::ClientCommand;

fn cmd(client: u32, sn: u64, op: u64) -> Entry {
    Entry::Cmd(ClientCommand { client, sn, op })
}

fn timed(entry: Entry, round: u64) -> Timed {
    Timed { entry, round }
}

fn pick<'a>(log: &'a ChunkSeq<Timed>, perm: &'a [u32], origin: u32) -> Chosen<'a> {
    Chosen { log, perm, origin }
}

#[test]
fn the_noop_predicate_holds_exactly_when_nothing_can_be_added() {
    let a = ChunkSeq::from_vec(vec![timed(cmd(1, 1, 1), 0), timed(cmd(2, 1, 1), 0)]);
    let pa = sorted_perm(&a);
    let same = vec![pick(&a, &pa, 0), pick(&a, &pa, 1), pick(&a, &pa, 2)];
    let cp = common_prefix_len(&same);
    assert_eq!(cp, a.len(), "identical logs agree over their whole length");
    assert!(union_is_noop(&same, cp, &[]));

    let append = [(
        ClientCommand {
            client: 9,
            sn: 1,
            op: 1,
        },
        0,
    )];
    assert!(
        !union_is_noop(&same, cp, &append),
        "an append can always add"
    );

    let longer = ChunkSeq::from_vec(vec![
        timed(cmd(1, 1, 1), 0),
        timed(cmd(2, 1, 1), 0),
        timed(cmd(3, 1, 1), 0),
    ]);
    let pl = sorted_perm(&longer);
    let mixed = vec![pick(&a, &pa, 0), pick(&longer, &pl, 1)];
    let cp = common_prefix_len(&mixed);
    assert_eq!(cp, 2);
    assert!(
        !union_is_noop(&mixed, cp, &[]),
        "a longer log carries a tail the union must walk"
    );
}

#[test]
fn the_overflow_fold_preserves_the_medians_tie_order() {
    let repeated = cmd(5, 1, 1);
    let median = ChunkSeq::from_vec(vec![timed(repeated.clone(), 7), timed(repeated.clone(), 9)]);
    // Non-identity perm over equal entries, so a reordering is visible.
    let carried: Vec<u32> = vec![1, 0];
    assert!(is_sorted_perm(&median, &carried));

    let fresh = cmd(6, 1, 1);
    let donor = ChunkSeq::from_vec(vec![
        timed(repeated.clone(), 7),
        timed(repeated.clone(), 9),
        timed(fresh, 3),
    ]);
    let pd = sorted_perm(&donor);
    let logs = vec![pick(&median, &carried, 0), pick(&donor, &pd, 1)];

    let mut merged = median.clone();
    let index = PermIndex::from_carried(&merged, &carried);
    let index = union_into(
        &mut merged,
        &logs,
        median.len(),
        &[],
        index,
        UnionStamp::FirstSighting,
    );

    assert_eq!(merged.len(), 3, "one fresh entry joins the two repeats");
    assert_eq!(
        index.into_perm(),
        vec![1, 0, 2],
        "the fold reordered the median's tie run instead of carrying it"
    );
}

// Forget line mid-chunk, where retained prefix, absolute frame and live window all disagree.
#[test]
fn the_gather_resolves_the_carried_perm_against_the_live_window() {
    let entries: Vec<Timed> = (0..3 * CHUNK as u64)
        .map(|i| timed(cmd(((3 * CHUNK as u64 - i) % 7) as u32, i, i), i))
        .collect();
    let mut log = ChunkSeq::from_vec(entries);
    let cut = CHUNK + 3;
    log.drop_front(cut);
    assert_ne!(
        log.start() % CHUNK as u64,
        0,
        "the forget line has to fall inside a chunk for this to have teeth"
    );
    assert_ne!(
        log.len(),
        log.retained_len(),
        "and the retained prefix has to differ from the live window"
    );

    let perm = sorted_perm(&log);
    let index = PermIndex::from_carried(&log, &perm);

    let mut expected: Vec<Entry> = log.iter().map(|t| t.entry.clone()).collect();
    expected.sort_unstable();
    assert_eq!(
        index.present(),
        expected,
        "the gather did not read the live window through the carried perm"
    );
}

fn cp_by_entry_walk(chosen: &[Chosen<'_>]) -> usize {
    let (first, rest) = chosen.split_first().expect("at least one chosen log");
    let shortest = rest.iter().fold(first.log.len(), |m, l| m.min(l.log.len()));
    (0..shortest)
        .take_while(|&i| rest.iter().all(|l| l.log.at(i) == first.log.at(i)))
        .count()
}

#[test]
fn the_chunk_floor_finds_the_same_prefix_the_entry_walk_does() {
    let n = CHUNK as u64 * 5;
    let body: Vec<Timed> = (0..n)
        .map(|i| timed(cmd((i % 5) as u32, i, i), i))
        .collect();

    let shared = ChunkSeq::from_vec(body.clone());
    let mut diverged = ChunkSeq::new();
    diverged.adopt(&shared);
    diverged.push(timed(cmd(99, 1, 1), 1));
    let mut other = ChunkSeq::new();
    other.adopt(&shared);
    other.push(timed(cmd(98, 1, 1), 1));
    assert!(
        diverged.shared_prefix_chunks(&other) > 0,
        "the fixture needs logs that really do share sealed chunks"
    );

    let copied = ChunkSeq::from_vec(body.clone());
    assert_eq!(copied.shared_prefix_chunks(&shared), 0);

    let mut nudged = ChunkSeq::new();
    nudged.adopt(&shared);
    nudged.drop_front(3);

    // Both forget 3 inside chunk 0: aligned, diverging in the tail inside the remainder.
    let sealed_only = ChunkSeq::from_vec(body[..CHUNK * 2].to_vec());
    let tail_pair: Vec<ChunkSeq<Timed>> = [97u32, 96]
        .iter()
        .map(|&c| {
            let mut s = ChunkSeq::new();
            s.adopt(&sealed_only);
            s.push(timed(cmd(c, 1, 1), 1));
            s.drop_front(3);
            s
        })
        .collect();
    let (ta, tb) = (&tail_pair[0], &tail_pair[1]);
    assert_eq!(ta.shared_prefix_chunks(tb), 2, "both hold both chunks");
    assert_eq!(
        ta.live_len_of_sealed_prefix(2),
        CHUNK * 2 - 3,
        "the chunks cover three fewer live entries than they hold"
    );
    assert_eq!(
        ta.len(),
        CHUNK * 2 - 3 + 1,
        "and each carries one tail entry"
    );
    let (pta, ptb) = (sorted_perm(ta), sorted_perm(tb));

    let p = |s: &ChunkSeq<Timed>| sorted_perm(s);
    let (pd, po, pc, pn, ps) = (p(&diverged), p(&other), p(&copied), p(&nudged), p(&shared));
    for set in [
        vec![pick(&diverged, &pd, 0), pick(&other, &po, 1)],
        vec![pick(&diverged, &pd, 0), pick(&copied, &pc, 1)],
        vec![pick(&diverged, &pd, 0), pick(&nudged, &pn, 1)],
        vec![pick(&nudged, &pn, 0), pick(&diverged, &pd, 1)],
        vec![
            pick(&diverged, &pd, 0),
            pick(&other, &po, 1),
            pick(&nudged, &pn, 2),
        ],
        vec![pick(&shared, &ps, 0), pick(&shared, &ps, 1)],
        vec![pick(&diverged, &pd, 0)],
        vec![pick(ta, &pta, 0), pick(tb, &ptb, 1)],
        vec![pick(tb, &ptb, 0), pick(ta, &pta, 1)],
    ] {
        assert_eq!(
            common_prefix_len(&set),
            cp_by_entry_walk(&set),
            "the chunk floor and the entry walk disagreed"
        );
    }
}

fn union_by_full_walk(
    merged: &mut ChunkSeq<Timed>,
    chosen: &[Chosen<'_>],
    from: usize,
    appends: &[(ClientCommand, u64)],
    mut index: PermIndex,
) -> Vec<u32> {
    for t in chosen.iter().flat_map(|c| c.log.iter_at(from)) {
        if index.insert(&t.entry, merged.len() as u32) {
            merged.push(t.clone());
        }
    }
    for (x, sent_round) in appends {
        let entry = Entry::Cmd(*x);
        if index.insert(&entry, merged.len() as u32) {
            merged.push(Timed {
                entry,
                round: *sent_round,
            });
        }
    }
    MergedIndex {
        present: index.present,
        perm: index.perm,
        overflow: index.overflow,
    }
    .into_perm()
}

#[test]
fn the_union_skip_reaches_the_full_walks_answer() {
    let n = CHUNK as u64 * 4;
    let body: Vec<Timed> = (0..n)
        .map(|i| timed(cmd((i % 3) as u32, i, i), i))
        .collect();
    let extra = |c: u32| timed(cmd(c, 7, 7), 7);

    let base = ChunkSeq::from_vec(body.clone());
    let seeded = |c: u32, drop: usize| {
        let mut s = ChunkSeq::new();
        s.adopt(&base);
        s.push(extra(c));
        s.drop_front(drop);
        s
    };
    let donor = seeded(42, 0);
    let nudged = seeded(44, 3);
    let mut rich = ChunkSeq::from_vec(body[..5].to_vec());
    rich.extend((0..CHUNK as u64 * 2).map(|i| timed(cmd(70, i, i), i)));
    let mut dissenter = ChunkSeq::from_vec(body[..5].to_vec());
    dissenter.push(extra(43));

    let appends = [(
        ClientCommand {
            client: 55,
            sn: 2,
            op: 2,
        },
        9,
    )];

    // Median nudged too: a nudged donor against an unmoved median is merely misaligned.
    let donor_nudged = seeded(42, 3);

    for (med_drop, set) in [
        (0, vec![&donor, &dissenter]),
        (0, vec![&donor, &rich]),
        (0, vec![&donor, &rich, &dissenter]),
        (0, vec![&donor, &nudged, &rich, &dissenter]),
        (0, vec![&nudged, &rich]),
        (0, vec![&donor]),
        (3, vec![&donor_nudged, &dissenter]),
        (3, vec![&donor_nudged, &rich, &dissenter]),
    ] {
        let med = seeded(41, med_drop);
        let perms: Vec<Vec<u32>> = std::iter::once(&&med)
            .chain(set.iter())
            .map(|l| sorted_perm(l))
            .collect();
        let logs: Vec<Chosen<'_>> = std::iter::once(&&med)
            .chain(set.iter())
            .zip(perms.iter())
            .enumerate()
            .map(|(i, (l, p))| pick(l, p, i as u32))
            .collect();
        let cp = common_prefix_len(&logs);

        let mut rider = ChunkSeq::new();
        seed_with_median(&mut rider, &med);
        let ri = PermIndex::from_carried(&rider, &perms[0]);
        let rider_perm = union_into(
            &mut rider,
            &logs,
            cp,
            &appends,
            ri,
            UnionStamp::FirstSighting,
        )
        .into_perm();

        let mut full = ChunkSeq::new();
        seed_with_median(&mut full, &med);
        let fi = PermIndex::from_carried(&full, &perms[0]);
        let full_perm = union_by_full_walk(&mut full, &logs, cp, &appends, fi);

        assert_eq!(
            rider.to_vec(),
            full.to_vec(),
            "the skip changed which entries the union produced"
        );
        assert_eq!(
            rider_perm, full_perm,
            "the skip changed the index the merge hands on"
        );
    }
}

#[test]
fn the_sort_prefilter_orders_the_logs_exactly_as_the_plain_compare_does() {
    let n = CHUNK as u64 * 4;
    let body: Vec<Timed> = (0..n)
        .map(|i| timed(cmd((i % 3) as u32, i, i), i))
        .collect();
    let base = ChunkSeq::from_vec(body.clone());
    let seeded = |c: u32, drop: usize| {
        let mut s = ChunkSeq::new();
        s.adopt(&base);
        s.push(timed(cmd(c, 7, 7), 7));
        s.drop_front(drop);
        s
    };

    let mut shorter = ChunkSeq::new();
    shorter.adopt(&base);
    let mut early = ChunkSeq::from_vec(body[..5].to_vec());
    early.push(timed(cmd(80, 1, 1), 1));
    let independent = {
        let mut s = ChunkSeq::from_vec(body.clone());
        s.push(timed(cmd(41, 7, 7), 7));
        s
    };

    let logs = [
        seeded(41, 0),
        seeded(42, 0),
        seeded(40, 0),
        // Same forget line: aligned, differing only in the tail inside the remainder.
        seeded(43, 3),
        seeded(45, 3),
        shorter,
        early,
        independent,
    ];
    let perms: Vec<Vec<u32>> = logs.iter().map(sorted_perm).collect();

    for from in [0, 3, 5] {
        let build = || -> Vec<Chosen<'_>> {
            logs.iter()
                .zip(perms.iter())
                .enumerate()
                .map(|(i, (l, p))| pick(l, p, i as u32))
                .collect()
        };

        let mut riders = build();
        let median = order_and_pick_median(&mut riders, logs.len(), from, StampTie::IncludeRound);

        let mut plain = build();
        plain.sort_unstable_by(|a, b| a.log.iter_at(from).cmp(b.log.iter_at(from)));

        assert_eq!(
            riders.iter().map(|c| c.origin).collect::<Vec<_>>(),
            plain.iter().map(|c| c.origin).collect::<Vec<_>>(),
            "the prefilter and the plain compare disagreed on the order at from={from}"
        );
        assert_eq!(median, logs.len() / 2);

        // Pairwise verdicts, since a wrong `Equal` can still sort correctly.
        for (i, a) in logs.iter().enumerate() {
            for (j, b) in logs.iter().enumerate() {
                assert_eq!(
                    compare_from(a, b, from, StampTie::IncludeRound),
                    a.iter_at(from).cmp(b.iter_at(from)),
                    "prefilter disagreed with the plain compare on ({i},{j}) at from={from}"
                );
            }
        }
    }
}

#[test]
fn two_holders_of_equal_content_can_carry_different_valid_indexes() {
    let repeated = cmd(5, 1, 1);
    let log = ChunkSeq::from_vec(vec![timed(repeated.clone(), 7), timed(repeated.clone(), 9)]);
    let holder_a: Vec<u32> = vec![0, 1];
    let holder_b: Vec<u32> = vec![1, 0];
    assert!(is_sorted_perm(&log, &holder_a));
    assert!(is_sorted_perm(&log, &holder_b));
    assert_ne!(holder_a, holder_b);

    let logs = vec![pick(&log, &holder_a, 0), pick(&log, &holder_b, 1)];
    let cp = common_prefix_len(&logs);
    assert_eq!(cp, log.len());
    assert!(
        union_is_noop(&logs, cp, &[]),
        "the fast path fires on exactly this shape"
    );
    assert_ne!(
        logs[0].perm, logs[1].perm,
        "the two holders' indexes differ, so which one is adopted is observable"
    );
}

/// Sorted M = [a@0], [a@0, x@1] (median), [b@0, x@3]; an append re-sights x at round 5.
fn merge_resighting_a_median_entry(union: UnionStamp) -> Vec<Timed> {
    let (a, b, x) = (cmd(1, 1, 1), cmd(2, 1, 2), cmd(3, 1, 3));
    let logs = [
        ChunkSeq::from_vec(vec![timed(a.clone(), 0)]),
        ChunkSeq::from_vec(vec![timed(a.clone(), 0), timed(x.clone(), 1)]),
        ChunkSeq::from_vec(vec![timed(b, 0), timed(x, 3)]),
    ];
    let perms: Vec<Vec<u32>> = logs.iter().map(sorted_perm).collect();
    let chosen: Vec<Chosen<'_>> = logs
        .iter()
        .zip(&perms)
        .enumerate()
        .map(|(i, (l, p))| pick(l, p, i as u32))
        .collect();
    let appends = [(
        ClientCommand {
            client: 3,
            sn: 1,
            op: 3,
        },
        5,
    )];
    let (merged, _) = merge_onto_median(&chosen, 1, 0, &appends, union);
    merged.to_vec()
}

#[test]
fn every_union_stamp_keeps_the_median_whole_stamps_included() {
    for union in [UnionStamp::FirstSighting, UnionStamp::LastSighting] {
        let merged = merge_resighting_a_median_entry(union);
        assert_eq!(
            merged[..2],
            [timed(cmd(1, 1, 1), 0), timed(cmd(3, 1, 3), 1)],
            "{union:?} changed L'_i"
        );
    }
}

#[test]
fn last_sighting_still_restamps_an_entry_of_l_bar() {
    let (y, z) = (cmd(4, 1, 4), cmd(5, 1, 5));
    let logs = [
        ChunkSeq::from_vec(vec![timed(y.clone(), 2)]),
        ChunkSeq::from_vec(vec![timed(z.clone(), 0)]),
        ChunkSeq::from_vec(vec![timed(z.clone(), 0), timed(y.clone(), 6)]),
    ];
    let perms: Vec<Vec<u32>> = logs.iter().map(sorted_perm).collect();
    let chosen: Vec<Chosen<'_>> = logs
        .iter()
        .zip(&perms)
        .enumerate()
        .map(|(i, (l, p))| pick(l, p, i as u32))
        .collect();
    let stamp_of_y = |union| {
        let (merged, _) = merge_onto_median(&chosen, 1, 0, &[], union);
        merged
            .to_vec()
            .into_iter()
            .find(|t| t.entry == y)
            .map(|t| t.round)
    };
    assert_eq!(stamp_of_y(UnionStamp::FirstSighting), Some(2));
    assert_eq!(stamp_of_y(UnionStamp::LastSighting), Some(6));
}
