//! The incremental split-brain checker must agree with the reference
//! whole-sequence check (`prefixes_consistent`) in every round,
//! including the recovery path where state adoption rewrites (and can
//! shorten) a node's executed sequence.

use protocol::compact::{ClientCommand, Entry, SharedState};
use protocol::shared_state::{ExecSeq, ExecutedView};
use rand::{Rng, SeedableRng};
use sim::smr::{ClientModel, PrefixChecker, prefixes_consistent};

fn cmd(op: u64) -> Entry {
    Entry::Cmd(ClientCommand {
        client: op as u32,
        sn: 1,
        op,
    })
}

/// The population as the driver leaves it: everything below the frontier
/// forgotten, clamped per node at its own length — so a node lagging the
/// frontier keeps a longer tail than its peers.
fn truncated(snapshot: &[Vec<Entry>], frontier: u64) -> Vec<SharedState> {
    snapshot
        .iter()
        .map(|seq| {
            let mut s = SharedState::from_entries(seq.clone(), Default::default());
            s.forget_committed_prefix(frontier);
            s
        })
        .collect()
}

fn views(states: &[SharedState]) -> Vec<ExecutedView<'_>> {
    states.iter().map(SharedState::executed).collect()
}

/// Feed per-round snapshots to both checkers; verdicts must match each round.
fn parity(rounds: &[Vec<Vec<Entry>>]) {
    let n = rounds[0].len();
    let mut inc = PrefixChecker::new(n);
    let mut latched = true;
    for (r, snapshot) in rounds.iter().enumerate() {
        let reference = prefixes_consistent(snapshot);
        let states = truncated(snapshot, 0);
        latched = latched && inc.check_round(&views(&states), 0);
        assert_eq!(
            latched, reference,
            "round {r}: incremental {latched} vs reference {reference}"
        );
        if !reference {
            return; // both latched — the sim stops trusting the run here
        }
    }
}

/// The same parity over the (offset, tail) representation, with a frontier
/// lagging the longest sequence by `lag` rounds' worth of entries.
///
/// Only shapes that STAY consistent are pinned this way, and that is a real
/// limit rather than test convenience: once a prefix is forgotten, a divergence
/// inside it is invisible to every reader, so a truncated run can only be
/// trusted about what it still holds. Runs that lose the latch keep their full
/// history from that round on, which is what makes the limit safe.
fn truncated_parity(rounds: &[Vec<Vec<Entry>>], lag: u64) {
    let n = rounds[0].len();
    let mut inc = PrefixChecker::new(n);
    let mut frontier = 0u64;
    for (r, snapshot) in rounds.iter().enumerate() {
        assert!(
            prefixes_consistent(snapshot),
            "round {r}: this harness only pins consistent shapes"
        );
        let states = truncated(snapshot, frontier);
        let offsets: Vec<u64> = states.iter().map(SharedState::executed_offset).collect();
        assert!(
            offsets.iter().all(|&o| o <= frontier),
            "round {r}: an offset {offsets:?} passed the frontier {frontier}"
        );
        assert!(
            inc.check_round(&views(&states), frontier),
            "round {r}: a consistent round was flagged at frontier {frontier}"
        );
        let longest = snapshot.iter().map(Vec::len).max().unwrap_or(0) as u64;
        frontier = frontier.max(longest.saturating_sub(lag));
    }
}

#[test]
fn consistent_growth_stays_safe() {
    parity(&[
        vec![vec![], vec![]],
        vec![vec![cmd(1)], vec![]],
        vec![vec![cmd(1), cmd(2)], vec![cmd(1)]],
        vec![vec![cmd(1), cmd(2)], vec![cmd(1), cmd(2)]],
    ]);
}

#[test]
fn transposed_suffix_is_flagged() {
    parity(&[
        vec![vec![cmd(1)], vec![cmd(1)]],
        vec![vec![cmd(1), cmd(2), cmd(3)], vec![cmd(1), cmd(3), cmd(2)]],
    ]);
}

#[test]
fn divergence_between_new_entries_of_the_same_round_is_flagged() {
    parity(&[
        vec![vec![], vec![]],
        vec![vec![cmd(1), cmd(2)], vec![cmd(1), cmd(9)]],
    ]);
}

#[test]
fn adoption_shrink_consistent_stays_safe() {
    // node 1 adopts a laggard peer's state: executed shortens, content agrees.
    parity(&[
        vec![vec![cmd(1), cmd(2), cmd(3)], vec![cmd(1), cmd(2), cmd(3)]],
        vec![vec![cmd(1), cmd(2), cmd(3)], vec![cmd(1)]],
        vec![vec![cmd(1), cmd(2), cmd(3), cmd(4)], vec![cmd(1), cmd(2)]],
    ]);
}

#[test]
fn adoption_shrink_with_divergent_content_is_flagged() {
    parity(&[
        vec![vec![cmd(1), cmd(2)], vec![cmd(1), cmd(2)]],
        vec![vec![cmd(1), cmd(2)], vec![cmd(9)]],
    ]);
}

#[test]
fn late_join_from_empty_verifies_full_content() {
    // a node that stayed empty then jumps ahead of the canonical frontier
    parity(&[
        vec![vec![cmd(1)], vec![]],
        vec![vec![cmd(1)], vec![cmd(1), cmd(2), cmd(3)]],
        vec![
            vec![cmd(1), cmd(2), cmd(3), cmd(4)],
            vec![cmd(1), cmd(2), cmd(3)],
        ],
    ]);
}

// Alg 5, p. 26: a ⊥ server adopts S_j and commits on it in the same step, so
// its executed sequence can change below its old length without shrinking.
#[test]
fn adopt_then_commit_to_equal_length_is_flagged() {
    parity(&[
        vec![vec![cmd(1), cmd(2)], vec![cmd(1), cmd(2)]],
        vec![vec![cmd(1), cmd(2)], vec![cmd(3), cmd(2)]],
    ]);
}

#[test]
fn rewritten_tail_at_equal_length_is_flagged() {
    let base: Vec<Entry> = (1..=10).map(cmd).collect();
    let mut rewritten: Vec<Entry> = (1..=8).map(cmd).collect();
    rewritten.extend([cmd(100), cmd(101)]);
    parity(&[vec![base.clone(), base.clone()], vec![base, rewritten]]);
}

#[test]
fn every_adopt_shorter_then_regrow_shape_matches_the_reference() {
    for old_len in 1..=4u64 {
        for adopted_len in 0..old_len {
            for new_len in old_len..=old_len + 2 {
                let before: Vec<Entry> = (1..=old_len).map(cmd).collect();
                let mut after: Vec<Entry> = (1..=adopted_len).map(cmd).collect();
                after.extend((adopted_len..new_len).map(|p| cmd(1000 + p)));
                parity(&[
                    vec![before.clone(), before.clone()],
                    vec![before.clone(), after],
                ]);
            }
        }
    }
}

/// Reference over the whole run: every sequence seen at any round end is
/// prefix-comparable with every other, which is what the latch certifies.
fn extend_history(history: &mut Vec<Entry>, seqs: &[ExecSeq]) -> bool {
    for seq in seqs {
        let seq = seq.to_vec();
        let common = seq.len().min(history.len());
        if seq[..common] != history[..common] {
            return false;
        }
        history.extend_from_slice(&seq[common..]);
    }
    true
}

/// Sequences that persist across rounds, so the checker's skip over a prefix a
/// node still holds is exercised: Alg 5 adoption (S_i := S_j shares S_j's
/// chunks) plus same-round commits, against the reference every round.
#[test]
fn persistent_sequences_with_adoptions_match_the_reference() {
    let n = 4;
    let mut diverged = Vec::new();
    for seed in 0..300 {
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
        let mut seqs: Vec<ExecSeq> = vec![ExecSeq::new(); n];
        let mut inc = PrefixChecker::new(n);
        let mut history = Vec::new();
        let mut forks = 0u64;
        for round in 0..150 {
            for i in 0..n {
                if rng.random_range(0..5) == 0 {
                    seqs[i] = seqs[rng.random_range(0..n)].clone();
                }
                for _ in 0..rng.random_range(0..3) {
                    let next = if rng.random_range(0..300) == 0 {
                        forks += 1;
                        cmd(1_000_000 + forks)
                    } else {
                        cmd(seqs[i].end() + 1)
                    };
                    seqs[i].push(next);
                }
            }
            let reference = extend_history(&mut history, &seqs);
            let views: Vec<ExecutedView<'_>> = seqs.iter().map(ExecutedView::of).collect();
            assert_eq!(
                inc.check_round(&views, 0),
                reference,
                "seed {seed} round {round}"
            );
            if !reference {
                diverged.push(round);
                break;
            }
        }
    }
    diverged.sort_unstable();
    assert!(
        diverged.len() > 100 && diverged[diverged.len() / 2] > 10,
        "the fixture must fork in most seeds, typically after many rounds: {diverged:?}"
    );
}

/// The position floor rides on this: the canonical length only ever grows,
/// and only over content some node had already executed in a checked round —
/// an adoption shrink must not walk it back.
#[test]
fn canonical_length_is_the_high_water_mark_of_checked_content() {
    let mut inc = PrefixChecker::new(2);
    let round = |inc: &mut PrefixChecker, seqs: Vec<Vec<Entry>>| {
        let states = truncated(&seqs, 0);
        inc.check_round(&views(&states), 0)
    };
    assert_eq!(inc.canonical_len(), 0);
    assert!(round(&mut inc, vec![vec![cmd(1), cmd(2)], vec![cmd(1)]]));
    assert_eq!(inc.canonical_len(), 2);
    assert!(round(&mut inc, vec![vec![cmd(1)], vec![cmd(1)]]));
    assert_eq!(
        inc.canonical_len(),
        2,
        "an adoption shrink must not lower it"
    );
    assert!(round(
        &mut inc,
        vec![vec![cmd(1), cmd(2), cmd(3)], vec![cmd(1)]]
    ));
    assert_eq!(inc.canonical_len(), 3);
}

/// E1's shape: the frontier is one number for the round, but a node that has
/// not caught up to it keeps its own shorter offset — so offsets are NOT
/// uniform, and the checker has to compare each tail from its own base.
#[test]
fn truncated_representation_matches_the_reference_on_consistent_shapes() {
    for lag in 1..=3 {
        truncated_parity(
            &[
                vec![vec![], vec![]],
                vec![vec![cmd(1)], vec![]],
                vec![vec![cmd(1), cmd(2)], vec![cmd(1)]],
                vec![vec![cmd(1), cmd(2), cmd(3)], vec![cmd(1), cmd(2)]],
                vec![
                    vec![cmd(1), cmd(2), cmd(3), cmd(4)],
                    vec![cmd(1), cmd(2), cmd(3), cmd(4)],
                ],
            ],
            lag,
        );
        // An adoption shrink at a non-zero offset: node 1 drops back to a
        // length below the frontier, so its offset clamps to its own length.
        truncated_parity(
            &[
                vec![vec![cmd(1), cmd(2), cmd(3)], vec![cmd(1), cmd(2), cmd(3)]],
                vec![vec![cmd(1), cmd(2), cmd(3)], vec![cmd(1)]],
                vec![
                    vec![cmd(1), cmd(2), cmd(3), cmd(4)],
                    vec![cmd(1), cmd(2), cmd(3)],
                ],
            ],
            lag,
        );
        // A node that stayed empty then jumps ahead of the frontier.
        truncated_parity(
            &[
                vec![vec![cmd(1), cmd(2)], vec![]],
                vec![vec![cmd(1), cmd(2)], vec![cmd(1), cmd(2), cmd(3)]],
            ],
            lag,
        );
    }
}

/// The oracle keeps its teeth on everything still held: a divergence above the
/// frontier is flagged exactly as it is without truncation.
#[test]
fn a_divergence_above_the_frontier_is_still_flagged() {
    let mut inc = PrefixChecker::new(2);
    // The frontier can never lead the canonical order, so build it first.
    let clean = vec![
        vec![cmd(1), cmd(2), cmd(3), cmd(4)],
        vec![cmd(1), cmd(2), cmd(3), cmd(4)],
    ];
    let states = truncated(&clean, 0);
    assert!(inc.check_round(&views(&states), 0));
    let states = truncated(&clean, 2);
    assert!(inc.check_round(&views(&states), 2));
    let forked = vec![
        vec![cmd(1), cmd(2), cmd(3), cmd(4), cmd(5)],
        vec![cmd(1), cmd(2), cmd(3), cmd(4), cmd(9)],
    ];
    let states = truncated(&forked, 2);
    assert!(
        !inc.check_round(&views(&states), 2),
        "a split brain above the frontier must still latch"
    );
}

fn traffic_cell(t_commit: u64, max_rounds: usize) -> sim::smr::SmrScenario {
    sim::smr::SmrScenario {
        n: 64,
        seed: 950_000,
        cfg: sim::Config::default(),
        sigma: 1.0,
        proto: sim::smr::Proto::Compact {
            t_commit_rounds: t_commit,
        },
        injections: vec![],
        max_rounds,
        schedule: sim::BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: Some(vec![sim::smr::TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.0, 0.0, 0.0, 0.0, 1.0],
        }]),
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

/// Seed-pinned regression pin of the RNG stream and merge order, not a paper check: the cell
/// forks at a small T, which the paper's sufficient condition T > T_B + max{T_B, T_M} neither
/// predicts nor excludes (its constants are unknown).
#[test]
fn known_split_brain_cell_still_latches_false() {
    // E5 dispatch-1's aborting cell (diverges at round 278) truncated past it.
    let report = sim::smr::run_smr(&traffic_cell(21, 400));
    assert!(!report.safety_ok);
}

#[test]
fn known_clean_cell_stays_true() {
    let report = sim::smr::run_smr(&traffic_cell(31, 400));
    assert!(report.safety_ok);
}

#[test]
#[ignore = "slow: ~80 s; cargo test -p sim --test paper -- --ignored the_split_brain_is_a_finite_t_effect"]
fn the_split_brain_is_a_finite_t_effect_that_does_not_return_at_larger_t() {
    // Strong safety needs T > T_B + max{T_B, T_M} (p. 27): at the cell that forks at T = 21,
    // every larger T tried must stay clean.
    for t_commit in [41, 61, 81] {
        let report = sim::smr::run_smr(&traffic_cell(t_commit, 400));
        assert!(report.safety_ok, "T = {t_commit}");
    }
}
