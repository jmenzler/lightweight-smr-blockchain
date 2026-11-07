//! Def 1.4 Strong Safety and Def 1.5 Monotonicity oracles against from-scratch references
//! over the live compact states, and their separation from the no-fork latch `safety_ok`.

use protocol::compact::Entry;
use sim::BlockSchedule;
use sim::smr::{SmrReport, SmrScenario, SmrState};

fn scenario(spec: serde_json::Value) -> SmrScenario {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(spec).expect("spec parses");
    SmrScenario::try_from(spec).expect("valid scenario")
}

fn compact(n: usize, seed: u64, t: u64, beta: f64, pmf: &[f64], rounds: usize) -> SmrScenario {
    scenario(serde_json::json!({
        "n": n, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "compact", "t_commit_rounds": t},
        "injections": [], "max_rounds": rounds,
        "traffic": {"kind": "pmf", "arrivals_pmf": pmf},
        "schedule": {"kind": "fresh_per_round", "fraction": beta},
    }))
}

#[derive(Debug, Default, PartialEq)]
struct Reference {
    retractions: u64,
    rewrites: u64,
    first_retraction: Option<usize>,
    strong_rounds: u64,
    first_strong: Option<usize>,
    first_fork: Option<usize>,
}

/// Useful at the round's end, the population every Def 1.4 oracle reads: L_i ≠ ⊥ after the
/// round's step (recovery: after the between-window pass) and not blocked in that round.
fn useful_at_end(holds_log: &[bool], blocked: &[u32]) -> Vec<bool> {
    (0..holds_log.len())
        .map(|i| holds_log[i] && !blocked.contains(&(i as u32)))
        .collect()
}

/// `None` when no server is useful, else whether every useful server holds one sequence.
fn agree(useful: &[bool], seqs: &[Vec<Entry>]) -> Option<bool> {
    let mut members = (0..seqs.len()).filter(|&i| useful[i]);
    let first = members.next()?;
    Some(members.all(|i| seqs[i] == seqs[first]))
}

/// Steps the scenario like `run_smr` and recomputes both definitions from materialised sequences.
/// Also returns the rounds whose verdict the round-start population (L_i ≠ ⊥ when the round
/// began, not blocked in it) would have read differently.
fn reference(sc: &SmrScenario) -> (SmrReport, Reference, Vec<usize>) {
    let BlockSchedule::FreshPerRound { fraction } = sc.schedule else {
        panic!("fresh_per_round expected");
    };
    let mut state = SmrState::new_with_certs(
        sc.n,
        sc.cfg,
        sc.proto,
        sc.sigma,
        sc.seed,
        &sc.injections,
        sc.traffic.as_deref().unwrap_or(&[]),
        sc.client_model,
        sc.partition.as_deref(),
        false,
        sc.merge_policy,
        sc.repeated_commit,
    );
    let blocked = state.blocked_count(fraction);
    let mut out = Reference::default();
    let mut start_population_differs = Vec::new();
    let mut prev: Vec<Vec<Entry>> = vec![Vec::new(); sc.n];
    for round in 1..=sc.max_rounds {
        let held_at_start: Vec<bool> = state.compact_logs().iter().map(Option::is_some).collect();
        let status = state.step_sampled(blocked);
        let seqs = state.executed_seqs();
        for (p, cur) in prev.iter_mut().zip(&seqs) {
            if cur.len() < p.len() || cur[..p.len()] != p[..] {
                out.retractions += 1;
                out.rewrites += u64::from(cur.len() >= p.len());
                out.first_retraction.get_or_insert(round);
            }
            p.clone_from(cur);
        }
        let held_at_end: Vec<bool> = state.compact_logs().iter().map(Option::is_some).collect();
        let at_end = agree(&useful_at_end(&held_at_end, &status.blocked), &seqs);
        if at_end == Some(false) {
            out.strong_rounds += 1;
            out.first_strong.get_or_insert(round);
        }
        let at_start = agree(&useful_at_end(&held_at_start, &status.blocked), &seqs);
        if (at_start == Some(false)) != (at_end == Some(false)) {
            start_population_differs.push(round);
        }
        if out.first_fork.is_none() && !state.safety_ok() {
            out.first_fork = Some(round);
        }
        if status.dead.is_some() {
            break;
        }
    }
    (state.report(), out, start_population_differs)
}

fn assert_matches(sc: &SmrScenario) -> (SmrReport, Reference) {
    let (report, want, _) = reference(sc);
    let got = Reference {
        retractions: report.retractions,
        rewrites: report.retraction_rewrites,
        first_retraction: report.first_retraction_round,
        strong_rounds: report.strong_safety_violation_rounds,
        first_strong: report.strong_safety_first_violation,
        first_fork: want.first_fork,
    };
    assert_eq!(
        got, want,
        "n={} seed={}: oracle vs reference",
        sc.n, sc.seed
    );
    (report, want)
}

// Def 1.4, p. 7: a useful server lagging another is a Strong Safety violation the no-fork latch accepts.
#[test]
fn compact_strong_safety_sees_a_lag_the_no_fork_latch_accepts() {
    let mut lag_before_fork = 0;
    for seed in 7000..7004 {
        let (_, r) = assert_matches(&compact(100, seed, 10, 0.0, &[0.0, 0.0, 1.0], 60));
        if r.first_strong
            .is_some_and(|s| r.first_fork.is_none_or(|f| s < f))
        {
            lag_before_fork += 1;
        }
    }
    assert!(
        lag_before_fork > 0,
        "no seed showed Def 1.4 violated before a fork"
    );
}

// Def 1.5, p. 8: a retraction in a run that never forks across servers is still counted.
#[test]
fn compact_retraction_without_a_fork_is_counted() {
    let (report, r) = assert_matches(&compact(24, 3, 3, 0.25, &[0.0, 0.5, 0.5], 150));
    assert!(report.safety_ok, "precondition: the run never forks");
    assert!(r.retractions >= 1, "precondition: a server retracts");
}

// Def 1.5, p. 8 and p. 29: a fork that heals did so by rewriting executed prefixes.
#[test]
fn compact_heal_after_a_fork_counts_rewrites() {
    let (report, r) = assert_matches(&compact(
        64,
        970012,
        16,
        0.1,
        &[0.0, 0.0, 0.0, 0.0, 1.0],
        70,
    ));
    assert!(!report.safety_ok, "precondition: the run forks");
    assert!(r.rewrites >= 1, "the heal must show as a rewrite");
}

#[test]
fn clean_compact_cell_has_no_violation_of_either_definition() {
    let (report, r) = assert_matches(&compact(64, 11, 20, 0.0, &[0.0, 0.0, 1.0], 120));
    assert!(report.safety_ok);
    assert_eq!((r.retractions, r.strong_rounds), (0, 0));
}

#[test]
fn lean_and_full_runs_agree_on_both_oracles() {
    for sc in [
        compact(100, 7001, 10, 0.0, &[0.0, 0.0, 1.0], 60),
        compact(24, 3, 3, 0.25, &[0.0, 0.5, 0.5], 150),
    ] {
        let full = sim::smr::run_smr(&sc);
        let lean = sim::smr::run_smr_lean(&sc);
        let pick = |r: &SmrReport| {
            (
                r.retractions,
                r.retraction_rewrites,
                r.first_retraction_round,
                r.strong_safety_violation_rounds,
                r.strong_safety_first_violation,
            )
        };
        assert_eq!(pick(&full), pick(&lean), "seed {}", sc.seed);
    }
}

fn recovery(seed: u64, t: u64, beta: f64, mismatch: &str, repeated: &str) -> SmrScenario {
    scenario(serde_json::json!({
        "n": 32, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": t, "prefix_mismatch": mismatch,
                  "resend_until_acked": false},
        "repeated_commit": repeated,
        "injections": [], "max_rounds": 130,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": beta},
    }))
}

#[derive(Debug, Default)]
struct RecoveryReference {
    /// First violating round: every round, window ends only.
    first: [Option<usize>; 2],
    rounds: [u64; 2],
    /// Round-end retractions the oracle did not count.
    missed: Vec<usize>,
    /// Def 1.4 verdict at each window end, as [`agree`] reads it.
    boundary_agree: Vec<(usize, Option<bool>)>,
    /// Rounds whose verdict the unmasked population (L_i ≠ ⊥ alone) would have read differently.
    unmasked_differs: Vec<usize>,
}

/// Recovery reference at round ends, stepped like `run_smr` so the blocked set is known.
fn recovery_reference(sc: &SmrScenario) -> (SmrReport, RecoveryReference) {
    let sim::smr::Proto::Recovery {
        t_window_rounds, ..
    } = sc.proto
    else {
        panic!("recovery expected");
    };
    let fraction_in = |round: usize| match &sc.schedule {
        BlockSchedule::FreshPerRound { fraction } => *fraction,
        BlockSchedule::PerRoundFractions(fractions) => {
            fractions.get(round - 1).copied().unwrap_or(0.0)
        }
        other => panic!("fresh or per-round fractions expected, got {other:?}"),
    };
    let mut state = SmrState::new_with_certs(
        sc.n,
        sc.cfg,
        sc.proto,
        sc.sigma,
        sc.seed,
        &sc.injections,
        sc.traffic.as_deref().unwrap_or(&[]),
        sc.client_model,
        sc.partition.as_deref(),
        false,
        sc.merge_policy,
        sc.repeated_commit,
    );
    let mut out = RecoveryReference::default();
    let mut prev: Vec<Vec<Entry>> = vec![Vec::new(); sc.n];
    let mut last_count = 0u64;
    for round in 1..=sc.max_rounds {
        let status = state.step_sampled(state.blocked_count(fraction_in(round)));
        if status.failed() {
            break;
        }
        let round = state.round();
        let seqs = state.executed_seqs();
        let mut retracted = false;
        for (p, cur) in prev.iter_mut().zip(&seqs) {
            retracted |= cur.len() < p.len() || cur[..p.len()] != p[..];
            p.clone_from(cur);
        }
        let counted = state.retraction_counts().0 > last_count;
        last_count = state.retraction_counts().0;
        if retracted && !counted {
            out.missed.push(round);
        }
        let holds: Vec<bool> = (0..sc.n)
            .map(|i| {
                state
                    .recovery_node(i)
                    .expect("recovery")
                    .log_seq()
                    .is_some()
            })
            .collect();
        let at_end = agree(&useful_at_end(&holds, &status.blocked), &seqs);
        let unmasked = agree(&holds, &seqs);
        if (unmasked == Some(false)) != (at_end == Some(false)) {
            out.unmasked_differs.push(round);
        }
        let boundary = (round as u64).is_multiple_of(t_window_rounds);
        if boundary {
            out.boundary_agree.push((round, at_end));
        }
        for (slot, applies) in [true, boundary].into_iter().enumerate() {
            if applies && at_end == Some(false) {
                out.rounds[slot] += 1;
                out.first[slot].get_or_insert(round);
            }
        }
    }
    (state.report(), out)
}

/// `recovered_round` recomputed: the first window end whose useful servers agree (Def 1.4) and
/// from which every window end is clean (no reset vote, at least ⌈3n/4⌉ logs).
fn recovered_reference(report: &SmrReport, n: usize, r: &RecoveryReference) -> Option<usize> {
    if report.terminal.is_failed() {
        return None;
    }
    let rows = &report.recovery.as_ref().expect("recovery").rounds;
    let floor = (3 * n).div_ceil(4) as u32;
    let clean = |b: usize| rows[b - 1].reset == 0 && report.metrics[b - 1].nonbot_logs >= floor;
    r.boundary_agree.iter().find_map(|&(b, agreed)| {
        let clean_from_here = r
            .boundary_agree
            .iter()
            .all(|&(later, _)| later < b || clean(later));
        (agreed == Some(true) && clean_from_here).then_some(b)
    })
}

// Def 1.4, p. 7 + Lemma 6.7, p. 35: useful servers unequal after a window end, with no fork reported.
#[test]
fn recovery_strong_safety_matches_the_reference_every_round_and_at_window_ends() {
    let mut boundary_lag_without_fork = 0;
    for seed in 4420000..4420008u64 {
        let sc = recovery(seed, 13, 0.1, "abort", "execute");
        let (report, r) = recovery_reference(&sc);
        if report.terminal.is_failed() {
            continue;
        }
        assert_eq!(
            (
                [
                    report.strong_safety_first_violation,
                    report.strong_safety_boundary_first_violation
                ],
                [
                    report.strong_safety_violation_rounds,
                    report.strong_safety_boundary_violations
                ]
            ),
            (r.first, r.rounds),
            "seed {seed}"
        );
        assert_eq!(
            report.recovered_round(sc.n, 13),
            recovered_reference(&report, sc.n, &r),
            "seed {seed}: recovered_round"
        );
        // A subset only: the oracle also observes before each window end.
        assert!(
            r.missed.is_empty(),
            "seed {seed}: round-end retractions missed in {:?}",
            r.missed
        );
        if report.safety_ok && r.first[1].is_some() {
            boundary_lag_without_fork += 1;
        }
    }
    assert!(
        boundary_lag_without_fork > 0,
        "no seed showed unequal useful servers at a window end without a fork"
    );
}

// Draw-stream digests hash serialized reports; the oracle counters are derived and stay out.
#[test]
fn digests_ignore_the_oracle_counters() {
    let report = sim::smr::run_smr(&compact(100, 7001, 10, 0.0, &[0.0, 0.0, 1.0], 60));
    assert!(report.strong_safety_violation_rounds > 0, "precondition");
    let mut zeroed = report.clone();
    zeroed.retractions = 0;
    zeroed.retraction_rewrites = 0;
    zeroed.first_retraction_round = None;
    zeroed.strong_safety_violation_rounds = 0;
    zeroed.strong_safety_first_violation = None;
    zeroed.strong_safety_boundary_violations = 0;
    zeroed.strong_safety_boundary_first_violation = None;
    let json = |r: &SmrReport| {
        crate::digest_fields::without_derived_fields(&serde_json::to_string(r).expect("serializes"))
    };
    assert_eq!(json(&report), json(&zeroed));
}

// Def 1.4 read at the round's end: a server whose log went ⊥ in the round leaves the population.
#[test]
fn compact_strong_safety_reads_the_round_end_population() {
    let sc = compact(24, 3, 3, 0.25, &[0.0, 0.5, 0.5], 150);
    let (report, want, start_differs) = reference(&sc);
    assert!(
        start_differs.contains(&5),
        "precondition: the round-start population reads round 5 differently ({start_differs:?})"
    );
    assert_eq!(
        (
            report.strong_safety_violation_rounds,
            report.strong_safety_first_violation
        ),
        (want.strong_rounds, want.first_strong)
    );
}

/// n = 64, T = 13, rate 4, a 0.85 blocking burst in rounds 40-60 over a 0.1 background.
fn recovery_burst(seed: u64) -> SmrScenario {
    let mut sc = recovery(seed, 13, 0.1, "skip_boundary", "execute");
    sc.n = 64;
    sc.schedule = BlockSchedule::PerRoundFractions(
        (1..=130)
            .map(|r| if (40..=60).contains(&r) { 0.85 } else { 0.1 })
            .collect(),
    );
    sc
}

// Def 1.4 + Lemma 6.10, pp. 7, 37: recovery needs equal sequences after the pass, not equal
// lengths before it.
#[test]
fn recovered_round_reads_the_window_end_after_the_pass() {
    let sc = recovery_burst(12);
    let (report, r) = recovery_reference(&sc);
    let lengths_equal_before_pass = report.metrics[90]
        .useful_executed
        .is_some_and(|(lo, hi)| lo == hi);
    assert!(
        lengths_equal_before_pass && r.boundary_agree.contains(&(91, Some(false))),
        "precondition: at window end 91 the pre-pass lengths are equal but the useful \
         sequences after the pass are not ({:?})",
        r.boundary_agree
    );
    assert_eq!(
        report.recovered_round(sc.n, 13),
        recovered_reference(&report, sc.n, &r)
    );
}
