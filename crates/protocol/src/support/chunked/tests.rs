use super::*;
use crate::compact::Entry;

fn seq(range: std::ops::Range<u64>) -> ChunkSeq<u64> {
    ChunkSeq::from_vec(range.collect())
}

#[test]
fn every_construction_path_produces_the_same_extents() {
    let n = CHUNK as u64 * 3 + 7;
    let bulk = seq(0..n);

    let mut incremental = ChunkSeq::new();
    for i in 0..n {
        incremental.push(i);
    }

    let mut adopted = ChunkSeq::new();
    adopted.adopt(&seq(0..CHUNK as u64 * 2));
    adopted.extend(CHUNK as u64 * 2..n);

    for (label, got) in [("incremental", &incremental), ("adopted", &adopted)] {
        assert_eq!(got.sealed.len(), bulk.sealed.len(), "{label}: sealed count");
        assert_eq!(got.base_chunk, bulk.base_chunk, "{label}: base");
        assert_eq!(got.tail.len(), bulk.tail.len(), "{label}: tail");
        assert_eq!(got, &bulk, "{label}: content");
    }

    let cut = CHUNK as u64 + 3;
    let mut a = bulk.clone();
    let mut b = incremental.clone();
    let mut c = adopted.clone();
    for s in [&mut a, &mut b, &mut c] {
        s.drop_below(cut);
    }
    for (label, got) in [("incremental", &b), ("adopted", &c)] {
        assert_eq!(
            got.sealed.len(),
            a.sealed.len(),
            "{label}: sealed after drop"
        );
        assert_eq!(got.base_chunk, a.base_chunk, "{label}: base after drop");
        assert_eq!(got, &a, "{label}: content after drop");
    }
}

#[test]
fn a_front_drop_releases_whole_chunks_and_keeps_the_remainder() {
    let mut s = seq(0..CHUNK as u64 * 4);
    let held_before = s.retained_len();
    s.drop_below(CHUNK as u64 * 2 + 5);
    assert_eq!(s.start(), CHUNK as u64 * 2 + 5);
    assert_eq!(s.len(), CHUNK * 2 - 5);
    assert_eq!(s.retained_len(), held_before - 2 * CHUNK);
    assert_eq!(s.get(CHUNK as u64 * 2 + 5), Some(&(CHUNK as u64 * 2 + 5)));
    assert_eq!(s.iter().next(), Some(&(CHUNK as u64 * 2 + 5)));
}

#[test]
#[should_panic(expected = "below the dropped prefix")]
fn reading_below_the_line_panics_rather_than_guessing() {
    let mut s = seq(0..CHUNK as u64 * 2);
    s.drop_below(CHUNK as u64);
    let _ = s.get(CHUNK as u64 - 1);
}

#[test]
fn iteration_matches_the_flat_sequence_from_every_position() {
    let n = CHUNK as u64 * 3 + 11;
    let flat: Vec<u64> = (0..n).collect();
    let s = seq(0..n);
    for from in 0..=n + 2 {
        let want: Vec<u64> = flat[(from.min(n) as usize)..].to_vec();
        let got: Vec<u64> = s.iter_from(from).copied().collect();
        assert_eq!(got, want, "from {from}");
    }

    let cut = CHUNK as u64 + 9;
    let mut dropped = s.clone();
    dropped.drop_below(cut);
    for from in 0..=n {
        let lo = from.max(cut).min(n) as usize;
        let want: Vec<u64> = flat[lo..].to_vec();
        let got: Vec<u64> = dropped.iter_from(from).copied().collect();
        assert_eq!(got, want, "dropped, from {from}");
    }
}

#[test]
fn equality_ignores_how_much_each_side_dropped() {
    let mut long = seq(0..CHUNK as u64 * 3);
    long.drop_front(CHUNK);
    let short = ChunkSeq::from_vec((CHUNK as u64..CHUNK as u64 * 3).collect());
    assert_ne!(
        long.start(),
        short.start(),
        "the fixture needs unequal starts"
    );
    assert_eq!(long, short);
    assert_eq!(short, long);

    let mut different = short.clone();
    different.push(1234);
    assert_ne!(long, different);
}

// Intra-chunk forget: same chunk allocations, different entry at relative position 0.
#[test]
fn shared_chunks_are_claimed_only_between_logs_whose_frames_line_up() {
    let base = seq(0..CHUNK as u64 * 4);
    let mut nudged = base.clone();
    nudged.drop_front(3);

    assert!(
        Arc::ptr_eq(&base.sealed[0], &nudged.sealed[0]),
        "the fixture needs the same allocation on both sides"
    );
    assert_eq!(
        base.base_chunk, nudged.base_chunk,
        "and the same base chunk"
    );
    assert_ne!(base.start(), nudged.start(), "but a different live window");
    assert_ne!(
        base.at(0),
        nudged.at(0),
        "so relative position 0 is a different entry in each — which is the bug"
    );

    assert_eq!(
        base.shared_prefix_chunks(&nudged),
        0,
        "a chunk shared at unequal relative offsets agrees over nothing a walk can use"
    );
    assert_eq!(
        base.shared_prefix_chunks(&base.clone()),
        base.sealed.len(),
        "an aligned clone shares every sealed chunk by handle"
    );
}

#[test]
fn relative_access_tracks_the_live_window_across_a_drop() {
    let n = CHUNK as u64 * 3 + 11;
    let flat: Vec<u64> = (0..n).collect();
    let mut s = seq(0..n);
    for (rel, want) in flat.iter().enumerate() {
        assert_eq!(s.at(rel), Some(want), "rel {rel}");
    }
    assert_eq!(s.at(flat.len()), None);

    let cut = CHUNK + 9;
    s.drop_front(cut);
    assert_eq!(s.len(), flat.len() - cut);
    assert_eq!(
        s.start(),
        cut as u64,
        "the frame did not move, the window did"
    );
    for rel in 0..s.len() {
        assert_eq!(s.at(rel), Some(&flat[cut + rel]), "after drop, rel {rel}");
    }
    assert_eq!(
        s.iter_at(3).copied().collect::<Vec<_>>(),
        flat[cut + 3..].to_vec()
    );
}

#[test]
fn walk_comparisons_answer_what_the_flat_slices_answer() {
    let cases: [(Vec<u64>, Vec<u64>); 5] = [
        (
            (0..CHUNK as u64 * 2 + 5).collect(),
            (0..CHUNK as u64 * 2 + 5).collect(),
        ),
        (
            (0..CHUNK as u64 * 2 + 5).collect(),
            (0..CHUNK as u64 * 2 + 9).collect(),
        ),
        ((0..CHUNK as u64 * 3).collect(), (0..CHUNK as u64).collect()),
        (vec![1, 2, 3], vec![1, 2, 4]),
        (vec![], vec![7]),
    ];
    for (i, (a, b)) in cases.iter().enumerate() {
        let (sa, sb): (ChunkSeq<u64>, ChunkSeq<u64>) =
            (ChunkSeq::from_vec(a.clone()), ChunkSeq::from_vec(b.clone()));
        for from in 0..=a.len().min(b.len()) {
            assert_eq!(
                sa.iter_at(from).cmp(sb.iter_at(from)),
                a[from..].cmp(&b[from..]),
                "case {i}: order from {from}"
            );
        }
        let want = (0..a.len().min(b.len()))
            .take_while(|&j| a[j] == b[j])
            .count();
        let got = sa.iter().zip(sb.iter()).take_while(|(x, y)| x == y).count();
        assert_eq!(got, want, "case {i}: common prefix");
    }
}

#[test]
fn writing_through_a_shared_chunk_copies_it_first() {
    let donor = seq(0..CHUNK as u64 * 2);
    let mut successor = ChunkSeq::new();
    successor.adopt(&donor);
    assert_eq!(
        donor.chunk_ptrs().collect::<Vec<_>>(),
        successor.chunk_ptrs().collect::<Vec<_>>(),
        "the fixture must start out sharing"
    );

    *successor.make_mut_at(3) = 999;
    assert_eq!(successor.at(3), Some(&999));
    assert_eq!(donor.at(3), Some(&3), "the donor must be untouched");
    assert_ne!(
        donor.chunk_ptrs().next(),
        successor.chunk_ptrs().next(),
        "the written chunk must have been copied out of the sharing"
    );
    assert_eq!(
        donor.chunk_ptrs().nth(1),
        successor.chunk_ptrs().nth(1),
        "an untouched chunk must still be shared"
    );
}

#[test]
fn writing_an_unshared_entry_touches_no_allocation() {
    let mut s = seq(0..CHUNK as u64 * 2 + 3);
    let before: Vec<_> = s.chunk_ptrs().collect();
    *s.make_mut_at(1) = 42;
    *s.make_mut_at(CHUNK * 2 + 1) = 43;
    assert_eq!(s.at(1), Some(&42));
    assert_eq!(s.at(CHUNK * 2 + 1), Some(&43));
    assert_eq!(
        before,
        s.chunk_ptrs().collect::<Vec<_>>(),
        "a uniquely-held chunk is written in place"
    );
}

#[test]
fn adopting_shares_the_sealed_chunks_rather_than_copying_them() {
    let donor = seq(0..CHUNK as u64 * 3 + 5);
    let mut successor = ChunkSeq::new();
    successor.adopt(&donor);
    successor.push(9999);

    let donor_ptrs: Vec<_> = donor.chunk_ptrs().collect();
    let succ_ptrs: Vec<_> = successor.chunk_ptrs().collect();
    assert_eq!(
        donor_ptrs, succ_ptrs,
        "the successor must hold the donor's chunk allocations, not copies"
    );
    assert_eq!(donor.len(), CHUNK * 3 + 5, "the donor is untouched");
    assert_eq!(successor.len(), CHUNK * 3 + 6);
}

#[test]
fn sealing_keys_on_length_and_not_on_batching() {
    for batch in [1usize, 2, 7, CHUNK - 1, CHUNK, CHUNK + 1, CHUNK * 2] {
        let n = CHUNK * 3 + 13;
        let mut s: ChunkSeq<u64> = ChunkSeq::new();
        let all: Vec<u64> = (0..n as u64).collect();
        for part in all.chunks(batch) {
            s.extend(part.iter().copied());
        }
        assert_eq!(s.sealed_count(), n / CHUNK, "batch {batch}: sealed count");
        assert_eq!(s.to_vec(), all, "batch {batch}: content");
    }
}

fn entries(range: std::ops::Range<u64>) -> ChunkSeq<Entry> {
    range.map(Entry::Nop).collect()
}

#[test]
fn growth_and_forgetting_below_it_keep_a_sealed_mark() {
    let mut s = entries(0..CHUNK as u64 * 2 + 3);
    let mark = s.last_sealed().expect("two chunks are sealed");
    assert_eq!(mark.end(), CHUNK as u64 * 2);
    s.extend((100..100 + CHUNK as u64 * 2).map(Entry::Nop));
    assert!(s.holds(&mark), "appending keeps every sealed chunk");
    s.drop_below(CHUNK as u64 + 1);
    assert!(s.holds(&mark), "a drop below the mark's chunk keeps it");
    s.drop_below(CHUNK as u64 * 2);
    assert!(!s.holds(&mark), "a drop past the mark's chunk releases it");
}

#[test]
fn only_a_shared_allocation_holds_a_sealed_mark() {
    let s = entries(0..CHUNK as u64 * 2);
    let mark = s.last_sealed().expect("sealed");
    assert!(s.clone().holds(&mark), "a clone shares the chunks");
    assert!(
        !entries(0..CHUNK as u64 * 2).holds(&mark),
        "equal content in a fresh allocation proves nothing about the prefix"
    );
    assert!(!entries(0..CHUNK as u64).holds(&mark), "too short");
    assert!(entries(0..CHUNK as u64 - 1).last_sealed().is_none());
    let mut adopted = ChunkSeq::new();
    adopted.adopt(&s);
    assert!(adopted.holds(&mark));
}

/// Random append, forget, clone and adopt steps over a population: whenever one
/// sequence holds another's mark, both agree on every position below it they retain.
#[test]
fn holders_of_a_mark_agree_below_it() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(7);
    let mut pool: Vec<ChunkSeq<Entry>> = (0..6).map(|_| ChunkSeq::new()).collect();
    let mut marks: Vec<SealedMark<Entry>> = Vec::new();
    for _ in 0..4000 {
        let i = rng.random_range(0..pool.len());
        match rng.random_range(0..10) {
            0..=4 => {
                // A two-letter alphabet: equal-content chunks with different prefixes abound.
                for _ in 0..rng.random_range(1..=CHUNK + 2) {
                    pool[i].push(Entry::Nop(rng.random_range(0..2)));
                }
            }
            5 => {
                let upto = rng.random_range(0..=pool[i].end());
                pool[i].drop_below(upto);
            }
            6 => {
                let j = rng.random_range(0..pool.len());
                pool[i] = pool[j].clone();
            }
            7 => {
                let j = rng.random_range(0..pool.len());
                let mut s = ChunkSeq::new();
                s.adopt(&pool[j]);
                pool[i] = s;
            }
            8 => pool[i] = pool[i].iter().cloned().collect(),
            _ => marks.extend(pool[i].last_sealed()),
        }
        for mark in &marks {
            let holders: Vec<&ChunkSeq<Entry>> = pool.iter().filter(|s| s.holds(mark)).collect();
            for a in &holders {
                for b in &holders {
                    let lo = a.start().max(b.start());
                    for p in lo..mark.end() {
                        assert_eq!(a.get(p), b.get(p), "holders disagree at {p}");
                    }
                }
            }
        }
        marks.retain(|m| pool.iter().any(|s| s.holds(m)));
    }
}

/// Random push, forget, clone, adopt and write-through steps against plain `Vec`s: after every
/// step each sequence in the population equals its model, so a write through a shared chunk that
/// reached a sibling, or a drop that moved the wrong boundary, shows up at once.
#[test]
fn random_operations_agree_with_a_plain_vec_model() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(11);
    let mut pool: Vec<ChunkSeq<u64>> = vec![ChunkSeq::new(); 5];
    let mut model: Vec<(u64, Vec<u64>)> = vec![(0, Vec::new()); 5];
    for step in 0..6000u64 {
        let i = rng.random_range(0..pool.len());
        let j = rng.random_range(0..pool.len());
        let live = model[i].1.len();
        match rng.random_range(0..10) {
            0..=3 => {
                for _ in 0..rng.random_range(1..=CHUNK + 2) {
                    pool[i].push(step);
                    model[i].1.push(step);
                }
            }
            4 => {
                let upto = rng.random_range(0..=pool[i].end());
                pool[i].drop_below(upto);
                let (start, v) = &mut model[i];
                let new_start = upto.clamp(*start, *start + live as u64);
                v.drain(..(new_start - *start) as usize);
                *start = new_start;
            }
            5 => {
                pool[i] = pool[j].clone();
                model[i] = model[j].clone();
            }
            6 => {
                let mut fresh = ChunkSeq::new();
                fresh.adopt(&pool[j]);
                pool[i] = fresh;
                model[i] = model[j].clone();
            }
            7 | 8 if live > 0 => {
                let rel = rng.random_range(0..live);
                *pool[i].make_mut_at(rel) = step + 1_000_000;
                model[i].1[rel] = step + 1_000_000;
            }
            _ => {
                let n = rng.random_range(0..=live);
                pool[i].drop_front(n);
                model[i].1.drain(..n);
                model[i].0 += n as u64;
            }
        }
        for (s, (start, v)) in pool.iter().zip(&model) {
            assert_eq!((s.start(), s.len()), (*start, v.len()), "step {step}");
            assert_eq!(&s.to_vec(), v, "step {step}");
        }
    }
}

// Shared sealed chunks skip the entry comparison; the frame and the tail are still compared.
#[test]
fn represents_same_as_over_shared_and_unshared_chunks() {
    let n = CHUNK as u64 * 2 + 3;
    let a = seq(0..n);
    let shared = a.clone();
    assert!(
        a.chunk_ptrs().eq(shared.chunk_ptrs()),
        "fixture: the clone shares the sealed chunks"
    );
    assert!(a.represents_same_as(&shared));
    assert!(
        a.represents_same_as(&seq(0..n)),
        "equal content, own chunks"
    );

    let mut longer = a.clone();
    longer.push(n);
    assert!(!a.represents_same_as(&longer), "same chunks, longer tail");

    let mut other = seq(0..n);
    *other.make_mut_at(0) = n;
    assert!(!a.represents_same_as(&other), "a differing sealed entry");
}
