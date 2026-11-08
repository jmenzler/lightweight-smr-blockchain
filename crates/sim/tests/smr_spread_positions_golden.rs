//! Byte-identity tripwire for the per-command report stream, whose
//! `SpreadPoint`s carry the executed-position stats. The observation path
//! resolves those positions incrementally; these digests are frozen from the
//! front-scan implementation over the three shapes whose executed sequences
//! are hardest — a run that splits its brain mid-way, and two recovery runs
//! whose boundary rollbacks rewrite executed sequences.
//! Regenerate only as a deliberate, commit-noted act.

use sim::smr::{
    ClientModel, Injection, Proto, SmrReport, SmrScenario, SmrState, TrafficPhase, point_mass_pmf,
    run_smr,
};
use sim::{BlockSchedule, Config};

#[path = "fixtures/digest_fields.rs"]
mod digest_fields;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn fold(mut h: u64, s: &str) -> u64 {
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// FNV-1a over the serialized command reports, with the shape counts kept
/// separately so a collapsed-to-nothing regression cannot hide behind a hash.
fn digest(report: &SmrReport) -> (usize, usize, u64) {
    // Observer-derived landmarks are not part of the position stream.
    let commands: Vec<_> = report
        .commands
        .iter()
        .cloned()
        .map(|c| sim::smr::CommandReport {
            committed_round: None,
            injected_round: None,
            ..c
        })
        .collect();
    let json = serde_json::to_string(&commands).expect("commands serialize");
    let points = report
        .commands
        .iter()
        .map(|c| c.spread.points().len())
        .sum();
    (report.commands.len(), points, fold(FNV_OFFSET, &json))
}

/// E5 dispatch-1's aborting cell: sustained traffic under compact, diverging
/// at round 278 — the run spans both the pre-violation and post-violation
/// position paths.
#[test]
fn compact_split_brain_cell_positions_are_stable() {
    let s = SmrScenario {
        n: 64,
        seed: 950_000,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Compact {
            t_commit_rounds: 21,
        },
        injections: vec![],
        max_rounds: 400,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: Some(vec![TrafficPhase {
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
    };
    let report = run_smr(&s);
    assert!(!report.safety_ok, "the cell must still split its brain");
    assert_eq!(digest(&report), (1600, 38_605, 2_169_188_693_476_059_478));
}

/// A larger compact cell whose safety latch HOLDS.
///
/// The compact digest above is a small split-brain run, so the compact engine's
/// census was pinned only on a cell that stops being an experiment partway
/// through. This is the healthy shape: n = 128 under sustained load with
/// blocking, a commit window wide enough that the latch survives the horizon,
/// and therefore many live cursors open at once across a long run.
///
/// Both preconditions are asserted rather than assumed, so the cell cannot
/// quietly degenerate into a second split-brain run and end up pinning only its
/// own setup.
#[test]
fn loaded_compact_with_a_live_latch_positions_are_stable() {
    let s = SmrScenario {
        n: 128,
        seed: 970_400,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Compact {
            t_commit_rounds: 70,
        },
        injections: vec![],
        max_rounds: 600,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
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
    };
    let report = run_smr(&s);
    assert!(
        report.safety_ok,
        "the latch must hold, or this is a second split-brain cell rather than \
         the healthy shape the observer levers run on"
    );
    assert!(
        report
            .metrics
            .iter()
            .any(|m| m.min_executed_len < m.max_executed_len),
        "the executed lengths never spread, so no cursor ever had work to do"
    );
    assert!(
        report.metrics.iter().all(|m| m.blocked == 12),
        "floor(0.1*128) must block exactly 12 servers per round"
    );
    assert_eq!(digest(&report), (2400, 168_044, 3_617_169_964_104_270_869));
}

/// The rq2 `rounds-fixed` r1 arm in miniature: compact under β = 0.1 with
/// sustained rate-1 traffic. Blocked nodes go ⊥, so coverage almost never
/// latches and ⊥ requesters ask for shared state most rounds — the shape the
/// per-round log index and the lazy state clones are built for. Hashing every
/// step status pins the per-round `spreading` holder lists, which never reach
/// the report, alongside the metrics and glances.
#[test]
fn loaded_compact_under_blocking_step_stream_is_stable() {
    let mut state = SmrState::new(
        64,
        Config::default(),
        Proto::Compact {
            t_commit_rounds: 38,
        },
        1.0,
        975_000,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(1),
        }],
        ClientModel::Unique,
        None,
    );
    let mut h = FNV_OFFSET;
    let mut holder_entries = 0usize;
    for _ in 0..300 {
        let status = state.step_fraction(0.1);
        holder_entries += status
            .spreading
            .iter()
            .map(|s| s.holders.len())
            .sum::<usize>();
        h = fold(
            h,
            &digest_fields::without_derived_fields(
                &serde_json::to_string(&status).expect("status serializes"),
            ),
        );
    }
    assert_eq!((holder_entries, h), (30_749, 7_488_869_709_967_826_197));
    assert_eq!(
        digest(&state.report()),
        (300, 11_717, 6_258_403_322_804_743_246)
    );
}

fn recovery(n: usize, seed: u64, t_window: u64, injections: Vec<Injection>) -> SmrScenario {
    SmrScenario {
        n,
        seed,
        cfg: Config::default(),
        sigma: 5.0,
        proto: Proto::Recovery {
            t_window_rounds: t_window,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        injections,
        max_rounds: 40,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: None,
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

fn inj(round: usize, client: u32, op: u64) -> Injection {
    Injection {
        round,
        client,
        op,
        target: None,
    }
}

/// The full recovery arc of `golden_recovery_landmarks_are_stable`: all-⊥
/// spiral, reset arming, and a 56-node rollback cascade at boundary 50 that
/// re-commits an already-committed command.
#[test]
fn recovery_rollback_cascade_positions_are_stable() {
    let mut targets = vec![0.0; 31];
    for t in targets.iter_mut().take(31).skip(21) {
        *t = 0.6;
    }
    let mut s = recovery(64, 424_242, 10, vec![inj(2, 1, 7)]);
    s.max_rounds = 60;
    s.schedule = BlockSchedule::PerRoundSticky {
        background: 0.1,
        targets,
    };
    let report = run_smr(&s);
    assert_eq!(report.commands[0].executed_round, Some(50));
    assert_eq!(digest(&report), (1, 50, 9_474_469_677_102_058_681));
}

/// Sustained traffic through a recovery surge: many commands are pending at
/// every boundary, so the boundary pass answers executed-membership for all
/// of them, and the surge's rollbacks rewrite executed sequences underneath.
/// Hashing the step stream pins the recovery `spreading` lists, which gate on
/// the boundary pass's `executed_round` latch.
#[test]
fn loaded_recovery_through_a_surge_step_stream_is_stable() {
    let mut state = SmrState::new(
        64,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: 20,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        5.0,
        4242,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(2),
        }],
        ClientModel::Unique,
        None,
    );
    let mut h = FNV_OFFSET;
    let mut holder_entries = 0usize;
    for round in 1..=400 {
        // Warm up clean, spiral the population, then release.
        let fraction = if (140..=220).contains(&round) {
            0.6
        } else {
            0.05
        };
        let status = state.step_fraction(fraction);
        holder_entries += status
            .spreading
            .iter()
            .map(|s| s.holders.len())
            .sum::<usize>();
        h = fold(
            h,
            &digest_fields::without_derived_fields(
                &serde_json::to_string(&status).expect("status serializes"),
            ),
        );
    }
    let report = state.report();
    let rows = &report.recovery.as_ref().expect("recovery block").rounds;
    let rollbacks: u32 = rows.iter().map(|r| r.rollbacks).sum();
    let executed = report
        .commands
        .iter()
        .filter(|c| c.executed_round.is_some())
        .count();
    assert!(rollbacks > 0, "the surge must roll checkpoints back");
    assert!(executed > 0, "the boundary pass must latch commitments");
    assert_eq!(
        (rollbacks, executed, holder_entries, h),
        (61, 718, 2_657_913, 14_643_451_127_489_063_887)
    );
    assert_eq!(digest(&report), (800, 58_920, 16_816_833_026_232_476_530));
}

/// A surge/release arc crossing twelve boundaries with rollbacks at three of
/// them, two commands straddling the recovery.
#[test]
fn recovery_surge_release_positions_are_stable() {
    let mut targets = vec![0.0; 200];
    for t in targets.iter_mut().take(200).skip(120) {
        *t = 0.6;
    }
    let mut s = recovery(128, 0, 40, vec![inj(5, 1, 7), inj(330, 2, 9)]);
    s.max_rounds = 480;
    s.schedule = BlockSchedule::PerRoundSticky {
        background: 0.1,
        targets,
    };
    let report = run_smr(&s);
    let rows = &report.recovery.as_ref().expect("recovery block").rounds;
    assert!(
        [241, 281, 321].iter().any(|&r| rows[r - 1].rollbacks > 0),
        "the arc must go through the reset path"
    );
    assert!(
        report.metrics.iter().take(120).all(|m| m.blocked == 12),
        "background floor(0.1*128) must block exactly 12 servers"
    );
    assert!(
        report
            .metrics
            .iter()
            .skip(120)
            .take(80)
            .all(|m| (76..=88).contains(&m.blocked)),
        "sticky floor(0.6*128)=76 union background 12 must stay in [76,88]"
    );
    assert_eq!(digest(&report), (2, 229, 145_342_823_802_424_030));
}
