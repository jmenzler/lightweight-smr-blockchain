use protocol::compact::Entry;
use sim::smr::{
    ClientModel, Injection, Proto, SmrScenario, SmrState, SmrTerminal, TrafficPhase, run_smr,
};
use sim::spec::SmrScenarioSpec;
use sim::{BlockSchedule, BlockTarget, BlockWindow, Config};

fn scenario(n: usize, seed: u64, t_window: u64, injections: Vec<Injection>) -> SmrScenario {
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

fn window(start_round: usize, rounds: usize, target: BlockTarget) -> BlockWindow {
    BlockWindow {
        start_round,
        rounds,
        target,
    }
}

// --- the run never reads Dead: all-⊥ is the expected mid-surge state (p. 29) ---

#[test]
fn all_bot_population_keeps_running_and_never_reads_dead() {
    // The same shape reads Dead at round 1 under extended
    // (undelivered_command_stays_pending_and_all_bot_terminates) — recovery
    // must instead idle through the spiral to the horizon.
    let mut s = scenario(32, 5, 10, vec![]);
    s.max_rounds = 50;
    s.schedule = BlockSchedule::Windows(vec![window(3, 48, BlockTarget::SampleFraction(1.0))]);
    let report = run_smr(&s);
    assert!(matches!(report.terminal, SmrTerminal::Ran { rounds: 50 }));
    assert_eq!(report.metrics.len(), 50, "one row per round, none skipped");
    for (i, m) in report.metrics.iter().enumerate() {
        if i + 1 >= 3 {
            assert_eq!(m.nonbot_logs, 0, "round {}: spiral holds", i + 1);
        }
    }
    let rec = report.recovery.expect("recovery block present");
    assert_eq!(rec.rounds.len(), 50);
}

#[test]
fn same_seed_replays_identically_for_a_surge_and_release_scenario() {
    // Proves determinism of the staged contract ((a0)+(a)–(f) + RNG-free
    // boundary pass); the golden test is what pins the order itself.
    let mut s = scenario(64, 42, 10, vec![inj(2, 1, 7)]);
    s.max_rounds = 60;
    s.schedule = BlockSchedule::Windows(vec![window(25, 21, BlockTarget::SampleFraction(0.6))]);
    assert_eq!(run_smr(&s), run_smr(&s));
}

// --- client semantics: extended-style delivery-is-ack with the ⊥/blocked guard ---

#[test]
fn delivery_to_blocked_or_bot_server_is_not_an_ack_under_recovery() {
    // Twin of the extended suite's shape: the round-2 draw hits either a
    // blocked server or the ⊥ one — neither may ack. All four nodes are ⊥
    // with genesis checkpoints from round 2 on, so no log ever revives and
    // the command must stay undelivered to the horizon (recovery keeps
    // running where extended reads Dead).
    for seed in 0..20 {
        let mut s = scenario(4, seed, 10, vec![inj(2, 1, 7)]);
        s.schedule = BlockSchedule::Windows(vec![
            window(1, 1, BlockTarget::Nodes(vec![0])),
            window(2, 1, BlockTarget::Nodes(vec![1, 2, 3])),
        ]);
        let report = run_smr(&s);
        assert_eq!(
            report.commands[0].delivered_round, None,
            "seed {seed}: blocked or ⊥ target must not ack"
        );
        assert!(matches!(report.terminal, SmrTerminal::Ran { rounds: 40 }));
    }
}

// --- §6 commitment observation: the E9 measurement channel (Lemma 6.8) ---

#[test]
fn executed_round_lands_only_on_window_boundaries_and_orders_after_delivery() {
    for seed in 0..10 {
        let mut s = scenario(64, seed, 40, vec![inj(5, 1, 7)]);
        s.max_rounds = 200;
        s.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
        let report = run_smr(&s);
        let cmd = &report.commands[0];
        let delivered = cmd.delivered_round.expect("seed {seed}: delivers");
        let executed = cmd
            .executed_round
            .unwrap_or_else(|| panic!("seed {seed}: never executed"));
        assert_eq!(
            executed % 40,
            0,
            "seed {seed}: §6 commitment happens only at boundaries"
        );
        assert!(delivered < executed, "seed {seed}");
        assert_eq!(cmd.status, sim::smr::CommandStatus::Complete, "seed {seed}");
        let last = report.metrics.last().unwrap();
        assert_eq!(
            (last.min_executed_len, last.max_executed_len),
            (1, 1),
            "seed {seed}: everyone executed the command"
        );
    }
}

/// The boundary pass answers executed-membership from one index extended per
/// boundary. A command injected in a late window is only ever found in the
/// suffix that boundary added, so an index left un-extended (or rebuilt off a
/// stale prefix) would leave every later command uncommitted forever.
#[test]
fn commands_injected_window_after_window_each_commit_at_their_own_boundary() {
    let injections: Vec<Injection> = (0..5)
        .map(|i| inj(2 + 10 * i, 1 + i as u32, 7 + i as u64))
        .collect();
    for seed in 0..5 {
        let mut s = scenario(32, seed, 10, injections.clone());
        s.max_rounds = 90;
        s.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
        let report = run_smr(&s);
        let mut prev = 0;
        for cmd in &report.commands {
            let executed = cmd
                .executed_round
                .unwrap_or_else(|| panic!("seed {seed}: op {} never committed", cmd.op));
            assert_eq!(executed % 10, 0, "seed {seed}: op {} off-boundary", cmd.op);
            assert!(
                executed > prev && executed > cmd.injection_round,
                "seed {seed}: op {} committed at {executed}, previous at {prev}",
                cmd.op
            );
            prev = executed;
        }
        let last = report.metrics.last().unwrap();
        assert_eq!(
            (last.min_executed_len, last.max_executed_len),
            (5, 5),
            "seed {seed}: every node executed all five"
        );
    }
}

// --- boundary row semantics: re-arm/rollback ordering (driver twin of U1/U2) ---

#[test]
fn boundary_rows_pin_rearm_then_rollback_ordering() {
    // Full block rounds 3–12 straddling boundary 10, release at 13. The
    // released population is ⊥ with genesis checkpoints; the row signature
    // pins step order 1→3 and the pre-boundary row semantics: reset appears
    // at window START (row mT+1), never at window end.
    let mut s = scenario(32, 11, 10, vec![]);
    s.max_rounds = 35;
    s.schedule = BlockSchedule::Windows(vec![window(3, 10, BlockTarget::SampleFraction(1.0))]);
    let report = run_smr(&s);
    let rows = report.recovery.expect("recovery block").rounds;
    let at = |round: usize| rows[round - 1];

    assert_eq!(at(10).bot_r, 32, "row 10 is pre-boundary: still ⊥");
    assert_eq!(
        at(11).bot_r,
        32,
        "boundary-10 re-arm invisible: rounds 11–12 still blocked"
    );
    assert_eq!(at(20).bot_r, 32, "silent population stays ⊥ to boundary 20");
    assert_eq!(
        (at(21).reset, at(21).bot_r, at(21).rollbacks),
        (32, 0, 0),
        "boundary 20 armed reset via step 3 — step 1 did NOT fire on ⊥ entry"
    );
    assert_eq!(
        at(31).rollbacks,
        32,
        "boundary 30 entered in Reset → step 1 fires; genesis P = ⊥ keeps the log ⊥"
    );
    assert_eq!(
        at(31).reset,
        32,
        "step 3 re-arms after the genesis rollback"
    );
}

// --- boundary extras: rollback depth, checkpoint fork count, aged |P| ---

#[test]
fn genesis_rollback_depth_equals_the_boundary_window_index() {
    // Blocked solid through round 39: every node sits at ⊥ with its genesis
    // checkpoint (W = 0). Boundary 40 re-arms them to Reset, which makes them
    // answerable again, so boundary 50 is the first one ENTERED in Reset —
    // step 1 fires and restores W = 0 from window index 5.
    let mut s = scenario(16, 21, 10, vec![]);
    s.max_rounds = 55;
    s.schedule = BlockSchedule::Windows(vec![window(1, 39, BlockTarget::SampleFraction(1.0))]);
    let report = run_smr(&s);
    let rows = report.recovery.expect("recovery block").rounds;
    let at = |round: usize| rows[round - 1];

    assert_eq!((at(51).rollbacks, at(51).rollback_depth), (16, 5));
    assert_eq!(
        at(50).rollback_depth,
        0,
        "row 50 is pre-boundary: the latch lands on row 51"
    );
    assert_eq!(
        (at(31).rollbacks, at(31).rollback_depth),
        (0, 0),
        "⊥ nodes re-arm at boundary 30 without rolling back"
    );
}

#[test]
fn fault_free_boundaries_report_a_single_unforked_checkpoint() {
    let mut s = scenario(16, 9, 10, vec![]);
    s.max_rounds = 45;
    let report = run_smr(&s);
    let rows = report.recovery.expect("recovery block").rounds;

    for (i, row) in rows.iter().enumerate() {
        let round = i + 1;
        let carries = round > 1 && (round - 1) % 10 == 0;
        assert_eq!(
            row.cp_fork_k,
            u32::from(carries),
            "row {round}: the fork count is boundary-latched, 0 elsewhere"
        );
    }
}

#[test]
fn aged_prefix_length_alternates_on_a_quiet_run() {
    // With no traffic every log stays a single no-op, so P alternates:
    // boundary 10 ages the genesis no-op into P; boundary 20 executes and
    // drains it, leaving a fresh no-op that is not yet T rounds old; boundary
    // 30 ages that one; and so on.
    let mut s = scenario(16, 9, 10, vec![]);
    s.max_rounds = 45;
    let report = run_smr(&s);
    let rows = report.recovery.expect("recovery block").rounds;
    let at = |round: usize| rows[round - 1];

    assert_eq!(
        [
            at(11).max_cp_p_len,
            at(21).max_cp_p_len,
            at(31).max_cp_p_len,
            at(41).max_cp_p_len,
        ],
        [1, 0, 1, 0]
    );
    assert_eq!(at(20).max_cp_p_len, 0, "non-carrying rows stay zero");
}

// --- command observation: spread curve, coverage landmark, watched set ---

/// A run whose single command is covered, then lost to a genesis rollback,
/// then re-spread by the client's resend. Blocked solid 13–22 drives the
/// population to ⊥; boundary 30 re-arms it to Reset, so boundary 40 is the
/// first one entered in Reset and step 1 restores the boundary-10 checkpoint,
/// whose P holds no command. From round 41 the logs are alive and empty.
fn re_aged_resend_run() -> SmrScenario {
    let mut s = scenario(32, 21, 10, vec![inj(2, 1, 7)]);
    s.max_rounds = 80;
    s.schedule = BlockSchedule::Windows(vec![window(13, 10, BlockTarget::SampleFraction(1.0))]);
    s
}

#[test]
fn spread_curve_collapses_at_the_rollback_and_re_rises_on_the_resend() {
    let report = run_smr(&re_aged_resend_run());
    let spread = report.commands[0].spread.points();
    let at = |round: usize| {
        *spread
            .iter()
            .find(|p| p.round == round)
            .unwrap_or_else(|| panic!("no spread point for round {round}"))
    };

    assert_eq!(
        (at(12).useful_holders, at(12).useful_total),
        (32, 32),
        "covered everywhere before the block"
    );
    assert_eq!(
        (at(41).useful_holders, at(41).useful_total),
        (0, 32),
        "logs are back from the checkpoint, the command is not"
    );
    assert!(
        spread
            .iter()
            .any(|p| p.round > 41 && p.useful_holders == p.useful_total && p.useful_total == 32),
        "the resend re-spreads the command across the revived population"
    );
}

#[test]
fn coverage_landmark_latches_once_and_survives_being_rolled_back() {
    // Rollbacks legitimately un-cover a command, so the recovery driver must
    // latch all_logs_round without the extended driver's regression panic.
    let report = run_smr(&re_aged_resend_run());
    let cmd = &report.commands[0];
    let latched = cmd.all_logs_round.expect("covered before the block");
    assert!(latched <= 12, "latched pre-block, got {latched}");
    let after = cmd
        .spread
        .points()
        .iter()
        .find(|p| p.round == 41)
        .expect("spread point");
    assert_eq!(
        (after.useful_holders, after.useful_total),
        (0, 32),
        "coverage really is lost while the landmark stands"
    );
}

#[test]
fn watched_set_follows_commitment_not_coverage() {
    // The compact rule drops a command from `spreading` once it reaches every
    // log; under recovery that is premature — a rollback can take it back
    // out, so the watched set only clears on §6 commitment.
    let s = re_aged_resend_run();
    let mut st = sim::smr::SmrState::new(
        s.n,
        s.cfg,
        s.proto,
        s.sigma,
        s.seed,
        &s.injections,
        &[],
        ClientModel::Unique,
        None,
    );
    let mut holders: Vec<Option<usize>> = Vec::new();
    for round in 1..=s.max_rounds {
        let mask = vec![(13..=22).contains(&round); s.n];
        let status = st.step_masked(&mask);
        holders.push(
            status
                .spreading
                .iter()
                .find(|c| c.client == 1)
                .map(|c| c.holders.len()),
        );
    }
    let at = |round: usize| holders[round - 1];

    // Holders are post-step, so round 41 already shows the resend's
    // same-round amplify seed set — the entering-round logs are empty.
    assert_eq!(at(12), Some(32), "covered, still watched");
    assert_eq!(at(40), Some(0), "watched through the ⊥ spiral");
    assert!(
        at(41).is_some_and(|h| h > 0),
        "the resend re-seeds the watched set the round after the rollback"
    );
    let committed = st.report().commands[0]
        .executed_round
        .expect("commits after the re-spread");
    assert_eq!(
        at(committed + 1),
        None,
        "§6 commitment at round {committed} clears the watch"
    );
}

// --- checkpoint spreading within one window ("immediately adopts", p. 30) ---

#[test]
fn released_straggler_catches_up_mid_window_by_checkpoint_adoption() {
    // Node 0 misses the boundary-120 commit while blocked and is released
    // mid-window at 151. State can change mid-window ONLY through adoption,
    // so min_executed_len rejoining max at a non-boundary round proves the
    // adoption path drove the catch-up.
    let mut s = scenario(64, 3, 40, vec![inj(5, 1, 7)]);
    s.max_rounds = 170;
    s.schedule = BlockSchedule::Windows(vec![window(100, 51, BlockTarget::Nodes(vec![0]))]);
    let report = run_smr(&s);
    let m = &report.metrics;
    assert_eq!(
        m[121 - 1].max_executed_len,
        1,
        "boundary-120 commit is first visible in the post-boundary row 121"
    );
    assert_eq!(m[150 - 1].min_executed_len, 0, "straggler still behind");
    let caught = (152..160).find(|&r| {
        let row = m[r - 1];
        row.min_executed_len == row.max_executed_len && row.min_executed_len == 1
    });
    let r = caught.expect("straggler must catch up strictly inside the window");
    assert_ne!(r % 40, 0, "a NON-boundary round: adoption, not a commit");
    assert_eq!(m[158 - 1].min_executed_len, 1, "holds by row 158");
    assert!(report.safety_ok);
}

// --- E2E paper properties ---

#[test]
fn full_window_zero_surge_latches_reset_forever() {
    // A surge covering the entire window 0 leaves only genesis checkpoints
    // (s₀, ⊥, 0): every boundary rolls back to P = ⊥ and step 3 re-arms —
    // no log ever revives, nothing ever commits, despite a benign adversary
    // from round 13 on. Theorem 7's sharp edge: recovery presupposes at
    // least one post-genesis consensus checkpoint; the implementation must
    // latch (halt safely) rather than invent progress.
    for seed in [1, 2, 3] {
        let mut s = scenario(32, seed, 10, vec![inj(30, 1, 7)]);
        s.max_rounds = 100;
        s.schedule = BlockSchedule::Windows(vec![window(1, 12, BlockTarget::SampleFraction(1.0))]);
        let report = run_smr(&s);
        let rows = &report.recovery.as_ref().expect("recovery block").rounds;
        for (i, m) in report.metrics.iter().enumerate() {
            assert_eq!(m.nonbot_logs, 0, "seed {seed} round {}", i + 1);
            assert_eq!(m.max_executed_len, 0, "seed {seed} round {}", i + 1);
        }
        for boundary in (30..=100).step_by(10) {
            assert_eq!(
                rows[boundary - 1].reset,
                32,
                "seed {seed}: all-reset at boundary row {boundary}"
            );
        }
        for post_boundary in (31..=91).step_by(10) {
            assert_eq!(
                rows[post_boundary - 1].rollbacks,
                32,
                "seed {seed}: genesis rollback fires at every boundary (row {post_boundary})"
            );
        }
        assert_eq!(report.commands[0].delivered_round, None, "seed {seed}");
        assert!(matches!(report.terminal, SmrTerminal::Ran { rounds: 100 }));
        assert!(report.safety_ok);
    }
}

#[test]
fn death_spiral_halts_commits_and_checkpoints_until_the_horizon() {
    // Under a surge the goal is to HALT committing and checkpoint-making
    // (p. 30): a death spiral is quiescence, not corruption. The post-
    // boundary reset cohorts die within a few rounds (≪ T), so no server
    // ever enters a boundary in Reset — no rollback, no mint.
    for seed in 0..10 {
        let mut s = scenario(128, seed, 40, vec![inj(5, 1, 7)]);
        s.max_rounds = 360;
        s.schedule =
            BlockSchedule::Windows(vec![window(121, 240, BlockTarget::SampleFraction(0.5))]);
        let report = run_smr(&s);
        let rows = &report.recovery.as_ref().expect("recovery block").rounds;
        assert_eq!(
            report.commands[0].executed_round,
            Some(120),
            "seed {seed}: warmup commit landed at boundary 120"
        );
        for round in 140..=360 {
            let m = report.metrics[round - 1];
            assert_eq!(m.nonbot_logs, 0, "seed {seed} round {round}: spiral holds");
            assert_eq!(
                m.max_executed_len, 1,
                "seed {seed} round {round}: no commits"
            );
            assert_eq!(
                rows[round - 1].max_checkpoint_window,
                3,
                "seed {seed} round {round}: no mint past boundary 120"
            );
        }
        for post_boundary in [161, 201, 241, 281, 321] {
            assert_eq!(
                rows[post_boundary - 1].rollbacks,
                0,
                "seed {seed}: ⊥ boundary entry takes step 3 only (row {post_boundary})"
            );
        }
        for boundary in (160..=360).step_by(40) {
            assert_eq!(
                rows[boundary - 1].bot_r,
                128,
                "seed {seed}: all-⊥ at boundary row {boundary}"
            );
        }
        assert!(matches!(report.terminal, SmrTerminal::Ran { rounds: 360 }));
        assert!(report.safety_ok, "seed {seed}");
    }
}

#[test]
fn one_tenth_blocking_commits_every_command_within_two_windows_and_never_rolls_back() {
    // Lemma 6.8: under a benign (1/10-blocking) adversary each command is
    // pre-committed by the end of the next full window after delivery and
    // executed one window later; nobody ever rolls back and the checkpoint
    // cadence never skips a window.
    for seed in 0..20 {
        let mut s = scenario(
            64,
            seed,
            40,
            vec![inj(15, 1, 7), inj(55, 2, 9), inj(95, 3, 11)],
        );
        s.max_rounds = 280;
        s.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
        let report = run_smr(&s);
        let rows = &report.recovery.as_ref().expect("recovery block").rounds;
        for cmd in &report.commands {
            let delivered = cmd.delivered_round.expect("delivers under benign");
            let executed = cmd.executed_round.expect("executes under benign");
            assert_eq!(executed % 40, 0, "seed {seed} op {}", cmd.op);
            assert!(
                executed <= (delivered.div_ceil(40) + 2) * 40,
                "seed {seed} op {}: {executed} beyond the two-window bound after {delivered}",
                cmd.op
            );
            assert_eq!(cmd.status, sim::smr::CommandStatus::Complete);
        }
        for boundary in (40..=280).step_by(40) {
            assert_eq!(
                rows[boundary - 1].reset,
                0,
                "seed {seed}: no reset at boundary row {boundary}"
            );
            assert_eq!(
                rows[boundary - 1].max_checkpoint_window,
                (boundary as u64 / 40) - 1,
                "seed {seed}: cadence never skips (pre-boundary row {boundary})"
            );
        }
        for row in rows.iter() {
            assert_eq!(row.rollbacks, 0, "seed {seed}: no rollback under benign");
        }
        let last = report.metrics.last().unwrap();
        assert_eq!(
            (last.min_executed_len, last.max_executed_len),
            (3, 3),
            "seed {seed}"
        );
        assert!(report.safety_ok, "seed {seed}");
    }
}

#[test]
fn reset_and_no_reset_never_coexist_at_any_window_end() {
    // Lemma 6.1: at a T-window END (the pre-boundary row mT) reset and
    // no-reset never coexist; mixes with ⊥ and mid-window transients are
    // legal. Sound only because D4 froze "reset appears at window START".
    let n = 256;
    let shapes = |seed: u64| {
        let benign = {
            let mut s = scenario(n, seed, 40, vec![]);
            s.max_rounds = 240;
            s.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
            s
        };
        let oscillating = {
            let mut s = scenario(n, seed, 40, vec![]);
            s.max_rounds = 240;
            let mut targets = vec![0.0; 200];
            for t in targets.iter_mut().take(80).skip(39) {
                *t = 0.35;
            }
            for t in targets.iter_mut().take(200).skip(159) {
                *t = 0.35;
            }
            s.schedule = BlockSchedule::PerRoundSticky {
                background: 0.0,
                targets,
            };
            s
        };
        let surge_release = {
            let mut s = scenario(n, seed, 40, vec![]);
            s.max_rounds = 240;
            s.schedule =
                BlockSchedule::Windows(vec![window(60, 81, BlockTarget::SampleFraction(0.5))]);
            s
        };
        [benign, oscillating, surge_release]
    };
    for seed in 0..50 {
        let mut noreset_side_bit = false;
        let mut reset_side_bit = false;
        for s in shapes(seed) {
            let report = run_smr(&s);
            let rows = &report.recovery.as_ref().expect("recovery block").rounds;
            for boundary in (40..=240).step_by(40) {
                let row = rows[boundary - 1];
                assert!(
                    !(row.reset > 0 && row.noreset > 0),
                    "seed {seed} row {boundary}: reset ({}) and no-reset ({}) coexist at a window end",
                    row.reset,
                    row.noreset
                );
                if row.noreset >= (n as u32) * 9 / 10 && row.reset == 0 {
                    noreset_side_bit = true;
                }
                if row.reset > 0 {
                    reset_side_bit = true;
                }
            }
            assert!(report.safety_ok);
        }
        assert!(
            noreset_side_bit && reset_side_bit,
            "seed {seed}: the oracle must bite on both sides across the shapes"
        );
    }
}

#[test]
fn reset_and_no_reset_never_coexist_at_a_window_end_under_client_load() {
    // Lemma 6.1 again, now with one client command arriving every round, so the logs the
    // boundary commits are non-trivial; the checkpoint lineage (Lemma 6.5) must stay one
    // chain through every boundary of the run.
    let n = 32;
    let loaded = |seed: u64, schedule: BlockSchedule| {
        let mut s = scenario(n, seed, 36, vec![]);
        s.max_rounds = 324;
        s.traffic = Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: sim::smr::point_mass_pmf(1),
        }]);
        s.schedule = schedule;
        s
    };
    let mut surge_resets = 0;
    for seed in 0..8 {
        let shapes = [
            BlockSchedule::FreshPerRound { fraction: 0.1 },
            BlockSchedule::Windows(vec![window(150, 70, BlockTarget::SampleFraction(0.5))]),
        ];
        for (shape, schedule) in shapes.into_iter().enumerate() {
            let report = run_smr(&loaded(seed, schedule));
            let rec = report.recovery.as_ref().expect("recovery block");
            for boundary in (36..=324).step_by(36) {
                let row = rec.rounds[boundary - 1];
                assert!(
                    !(row.reset > 0 && row.noreset > 0),
                    "seed {seed} shape {shape} row {boundary}: reset ({}) and no-reset ({}) coexist",
                    row.reset,
                    row.noreset
                );
                if shape == 1 {
                    surge_resets += u64::from(row.reset > 0);
                }
            }
            assert!(
                rec.rounds.iter().all(|row| row.cp_fork_k <= 1),
                "seed {seed} shape {shape}: two checkpoints at the largest window"
            );
            assert!(report.safety_ok && rec.fork_ok, "seed {seed} shape {shape}");
            let committed = rec.rounds.iter().map(|row| row.max_checkpoint_window).max();
            assert!(
                committed > Some(3) && report.metrics[200].max_executed_len > 0,
                "seed {seed} shape {shape}: the load must produce commits before the surge ends"
            );
        }
    }
    assert!(
        surge_resets > 0,
        "the surge shape must exercise reset at some window end"
    );
}

#[test]
fn arbitrary_blocking_hammer_upholds_monotonicity_prefix_consistency_and_checkpoint_lineage() {
    // Safety only — arbitrary blocking includes never-recovering shapes, so
    // no liveness claims. Lemma 6.9 (monotonicity) + Lemma 6.5/Cor 6.6/6.7
    // (no same-W forks, S-chain extends) must hold unconditionally; the
    // engine's absorbing asserts and the per-node Lemma 6.2 debug_assert
    // must stay silent (a panic fails the run).
    use rand::{Rng, SeedableRng};
    for seed in 0..50 {
        let mut g = rand_chacha::ChaCha12Rng::seed_from_u64(seed);
        let targets: Vec<f64> = (0..27)
            .flat_map(|_| {
                let level = [0.0, 0.2, 0.4, 0.6, 0.8][g.random_range(0..5)];
                std::iter::repeat_n(level, 15)
            })
            .take(400)
            .collect();
        let mut rounds: Vec<usize> = Vec::new();
        while rounds.len() < 3 {
            let r = g.random_range(1..=300);
            if !rounds.contains(&r) {
                rounds.push(r);
            }
        }
        let mut s = scenario(
            128,
            seed,
            40,
            vec![
                inj(rounds[0], 1, 7),
                inj(rounds[1], 2, 9),
                inj(rounds[2], 3, 11),
            ],
        );
        s.max_rounds = 400;
        s.schedule = BlockSchedule::PerRoundSticky {
            background: 0.0,
            targets,
        };
        let report = run_smr(&s);
        assert!(report.safety_ok, "seed {seed}: split brain");
        let rec = report.recovery.expect("recovery block");
        assert!(rec.fork_ok, "seed {seed}: checkpoint lineage forked");
        assert!(matches!(report.terminal, SmrTerminal::Ran { rounds: 400 }));
    }
}

#[test]
fn gate_satisfying_release_recovers_within_three_windows() {
    // Lemma 6.10 / Theorem 7 sanity bound (E9 measures the real
    // distribution): warmup commits op A and arms non-genesis checkpoints;
    // the surge spirals everyone; the release satisfies the hypothesis gate
    // (useful ≈ 0.9n ≥ n/4 sustained; holder fraction 0.9 ≥ 5/9), so a
    // valid commitment-problem solution returns within 3T = 120 rounds.
    for seed in 0..20 {
        let mut targets = vec![0.0; 200];
        for t in targets.iter_mut().take(200).skip(120) {
            *t = 0.6;
        }
        let mut s = scenario(128, seed, 40, vec![inj(5, 1, 7), inj(330, 2, 9)]);
        s.max_rounds = 480;
        s.schedule = BlockSchedule::PerRoundSticky {
            background: 0.1,
            targets,
        };
        let report = run_smr(&s);
        let rows = &report.recovery.as_ref().expect("recovery block").rounds;
        assert_eq!(
            report.commands[0].executed_round,
            Some(120),
            "seed {seed}: warmup commit"
        );
        assert!(
            [241, 281, 321].iter().any(|&r| rows[r - 1].rollbacks > 0),
            "seed {seed}: recovery must go through the reset path"
        );
        let boundaries: Vec<usize> = (240..=480).step_by(40).collect();
        let recovered = |b: usize| {
            let row = rows[b - 1];
            let m = report.metrics[b - 1];
            row.reset == 0
                && m.nonbot_logs >= 96
                && m.min_executed_len == m.max_executed_len
                && m.min_executed_len >= 1
        };
        let r_star = boundaries
            .iter()
            .copied()
            .find(|&b| recovered(b))
            .unwrap_or_else(|| panic!("seed {seed}: never recovered"));
        assert!(r_star <= 321, "seed {seed}: recovery too late ({r_star})");
        for &b in boundaries.iter().filter(|&&b| b >= r_star) {
            assert!(recovered(b), "seed {seed}: regressed at boundary row {b}");
        }
        let op_b = &report.commands[1];
        let executed = op_b.executed_round.expect("op B executes post-recovery");
        assert!(
            executed <= 330 + 120,
            "seed {seed}: liveness bound ({executed})"
        );
        assert_eq!(executed % 40, 0);
        assert_eq!(op_b.status, sim::smr::CommandStatus::Complete);
        assert!(report.safety_ok, "seed {seed}");
    }
}

#[test]
fn previously_blocked_minority_never_commits_a_divergent_sequence_after_block_reversal() {
    // The p. 29–30 core attack: let the majority commit while a minority M
    // is cut off (|M| = 12, 9.4% < 1/10 keeps phase B inside Lemma 6.8's
    // guarantee), then REVERSE the block so M alone runs (a surge, 12 <
    // n/4) and feed it a poison command. M must never commit a divergent
    // sequence; after full release the system re-solves the commitment
    // problem on the majority's history (Def 1.4 re-solved end to end). This reversal starves
    // M by construction; the one where the blocked group can progress is the next test.
    let m_nodes: Vec<u32> = (0..12).collect();
    let majority: Vec<u32> = (12..128).collect();
    for seed in 0..20 {
        let mut s = scenario(
            128,
            seed,
            40,
            vec![
                inj(5, 1, 101),
                inj(130, 2, 102),
                Injection {
                    round: 300,
                    client: 3,
                    op: 103,
                    target: Some(3),
                },
            ],
        );
        s.max_rounds = 720;
        s.schedule = BlockSchedule::Windows(vec![
            window(121, 160, BlockTarget::Nodes(m_nodes.clone())),
            window(281, 160, BlockTarget::Nodes(majority.clone())),
        ]);
        let report = run_smr(&s);
        let rows = &report.recovery.as_ref().expect("recovery block").rounds;
        let m = &report.metrics;

        // (1) Lemma 6.9's invariant surface, every round of the whole run.
        assert!(report.safety_ok, "seed {seed}: split brain");

        // (2) Phase B anti-vacuity: the majority really committed op B while
        // M sat frozen at [A].
        assert_eq!(
            (m[245 - 1].min_executed_len, m[245 - 1].max_executed_len),
            (1, 2),
            "seed {seed}: majority at [A,B], M frozen at [A]"
        );

        // (3) Phase C halt: nobody commits, mints, or rolls back; the poison
        // cannot even land while M's logs are ⊥.
        for round in 281..=440 {
            assert_eq!(
                m[round - 1].max_executed_len,
                2,
                "seed {seed} round {round}: phase C must be flat"
            );
            assert_eq!(
                rows[round - 1].max_checkpoint_window,
                7,
                "seed {seed} round {round}: no mint past boundary 280"
            );
        }
        for post_boundary in [321, 361, 401, 441] {
            assert_eq!(
                rows[post_boundary - 1].rollbacks,
                0,
                "seed {seed}: starved faction never enters a boundary in Reset (row {post_boundary})"
            );
        }

        // (4) Phase D recovery: through the reset path, then convergence on
        // the majority's history within 3T of release.
        let rb = [481, 521, 561]
            .into_iter()
            .find(|&r| rows[r - 1].rollbacks > 0)
            .unwrap_or_else(|| panic!("seed {seed}: no rollback by 441 + 3T"));
        let b_star = rb + 39;
        let row = rows[b_star - 1];
        assert_eq!(
            (row.reset, row.bot_r),
            (0, 0),
            "seed {seed}: boundary row {b_star} fully no-reset"
        );
        assert!(
            m[b_star - 1].min_executed_len == m[b_star - 1].max_executed_len
                && m[b_star - 1].min_executed_len >= 2,
            "seed {seed}: everyone converged on the majority's history by row {b_star}"
        );

        // (5) Full closure: the retried poison commits cleanly on the
        // re-established system — Def 1.4 re-solved.
        let op_c = &report.commands[2];
        let delivered = op_c.delivered_round.expect("op C delivers post-release");
        assert!(
            delivered > 440,
            "seed {seed}: M's ⊥ logs cannot accept C in phase C"
        );
        let executed = op_c.executed_round.expect("op C executes post-recovery");
        assert_eq!(executed % 40, 0, "seed {seed}");
        assert!(
            executed > 480,
            "seed {seed}: committed by the recovered system"
        );
        assert_eq!(op_c.status, sim::smr::CommandStatus::Complete);
        let last = m.last().unwrap();
        assert_eq!(
            (last.min_executed_len, last.max_executed_len),
            (3, 3),
            "seed {seed}: final history [A, B, C] everywhere"
        );
    }
}

#[test]
fn a_previously_blocked_group_that_can_progress_adopts_the_newest_checkpoint_and_forks_nothing() {
    // The p. 29-30 attack with a reversal that does NOT starve: B = 30 of 128 nodes (23%)
    // sit out while the other 98 commit op B; then 30 of those 98 are blocked, so the active
    // group B + 68 holders (98 nodes, 76%) can keep running. B must adopt the holders'
    // newest checkpoint instead of minting on its stale history, op C pinned to a B node
    // commits on top of [A, B], and no two checkpoints of the largest window ever differ.
    let blocked_first: Vec<u32> = (0..30).collect();
    let blocked_second: Vec<u32> = (98..128).collect();
    for seed in 0..6 {
        let mut s = scenario(
            128,
            seed,
            40,
            vec![
                inj(5, 1, 101),
                inj(130, 2, 102),
                Injection {
                    round: 300,
                    client: 3,
                    op: 103,
                    target: Some(3),
                },
            ],
        );
        s.max_rounds = 720;
        s.schedule = BlockSchedule::Windows(vec![
            window(121, 160, BlockTarget::Nodes(blocked_first.clone())),
            window(281, 160, BlockTarget::Nodes(blocked_second.clone())),
        ]);
        let report = run_smr(&s);
        let rec = report.recovery.as_ref().expect("recovery block");
        let (m, rows) = (&report.metrics, &rec.rounds);

        assert!(report.safety_ok, "seed {seed}: split brain");
        assert!(rec.fork_ok, "seed {seed}: checkpoint lineage forked");
        assert!(
            rows.iter()
                .all(|row| row.cp_fork_k <= 1 && row.rollbacks == 0),
            "seed {seed}: two checkpoints at the largest window, or a rollback"
        );
        assert!(
            rows.windows(2)
                .all(|w| w[1].max_checkpoint_window >= w[0].max_checkpoint_window),
            "seed {seed}: the newest checkpoint window went backwards"
        );

        // The first group really sat out the commit of op B at [A].
        assert_eq!(
            (m[245 - 1].min_executed_len, m[245 - 1].max_executed_len),
            (1, 2),
            "seed {seed}: the others at [A, B], the blocked group frozen at [A]"
        );
        // After the reversal the active group sustains itself and the first group has caught
        // up to [A, B] long before op C could commit.
        for round in 320..=440 {
            assert!(
                m[round - 1].nonbot_logs >= 80,
                "seed {seed} round {round}: the active group must stay alive, not spiral"
            );
            assert!(
                m[round - 1].min_executed_len >= 2,
                "seed {seed} round {round}: the first group is back at [A, B]"
            );
        }

        let op_c = &report.commands[2];
        assert_eq!(
            op_c.status,
            sim::smr::CommandStatus::Complete,
            "seed {seed}"
        );
        assert!(op_c.executed_round.is_some_and(|r| r > 300), "seed {seed}");
        let last = m.last().unwrap();
        assert_eq!(
            (last.min_executed_len, last.max_executed_len),
            (3, 3),
            "seed {seed}: final history [A, B, C] everywhere"
        );
    }
}

#[test]
fn golden_recovery_landmarks_are_stable() {
    // Frozen values pin the stage order AND the boundary-after-(f) timing:
    // same-seed replay passes under any consistent reorder, only these
    // landmarks catch one. The scenario walks a full recovery arc — benign
    // warmup, 0.6 surge straddling boundary 30, all-⊥ spiral, reset arming
    // at boundary 40, rollback cascade at boundary 50 re-committing the
    // pre-committed command (executed_round = the rollback boundary itself).
    // Regenerate only as a deliberate, commit-noted act.
    let mut targets = vec![0.0; 31];
    for t in targets.iter_mut().take(31).skip(21) {
        *t = 0.6;
    }
    let mut s = scenario(64, 424242, 10, vec![inj(2, 1, 7)]);
    s.max_rounds = 60;
    s.schedule = BlockSchedule::PerRoundSticky {
        background: 0.1,
        targets,
    };
    let report = run_smr(&s);
    let rec = report.recovery.as_ref().expect("recovery block");
    let rows = &rec.rounds;
    let cmd = &report.commands[0];
    assert_eq!(cmd.delivered_round, Some(2));
    assert_eq!(cmd.executed_round, Some(50));
    let boundary = |b: usize| {
        let r = rows[b - 1];
        (r.noreset, r.reset, r.bot_r, r.max_checkpoint_window)
    };
    assert_eq!(boundary(10), (57, 0, 7, 0));
    assert_eq!(boundary(20), (58, 0, 6, 1));
    assert_eq!(boundary(30), (0, 0, 64, 2));
    assert_eq!(boundary(40), (0, 0, 64, 2));
    assert_eq!(boundary(50), (0, 56, 8, 2));
    assert_eq!(boundary(60), (58, 0, 6, 5));
    assert_eq!(
        [
            rows[31 - 1].rollbacks,
            rows[41 - 1].rollbacks,
            rows[51 - 1].rollbacks
        ],
        [0, 0, 56]
    );
    // The boundary extras ride the same latch; boundary 60 has no row to land
    // on, so the horizon truncates its badge.
    let extras = |b: usize| {
        let r = rows[b];
        (r.rollback_depth, r.cp_fork_k, r.max_cp_p_len)
    };
    assert_eq!(
        [extras(10), extras(20), extras(30), extras(40), extras(50)],
        [(0, 1, 1), (0, 1, 1), (0, 1, 1), (0, 1, 1), (3, 1, 0)]
    );
    let last = report.metrics.last().unwrap();
    assert_eq!((last.min_executed_len, last.max_executed_len), (1, 1));
    assert_eq!(last.nonbot_logs, 58);
    assert!(rec.fork_ok);
    assert!(report.safety_ok);
}

// --- recovered_round: E3's oracle as a per-run summary stat (E9 CSV column) ---

mod recovered_round {
    use sim::smr::{RecRoundMetrics, RecoveryReport, SmrReport, SmrRoundMetrics, SmrTerminal};

    /// A report whose per-round (nonbot_logs, reset, min_exec, max_exec)
    /// come from the given vector, the useful servers agreeing iff min_exec
    /// == max_exec; all other fields are irrelevant zeros.
    fn report(rows: &[(u32, u32, u32, u32)]) -> SmrReport {
        SmrReport {
            metrics: rows
                .iter()
                .map(
                    |&(nonbot_logs, _, min_executed_len, max_executed_len)| SmrRoundMetrics {
                        nonbot_logs,
                        blocked: 0,
                        useful: 0,
                        distinct_logs: 0,
                        max_log_len: 0,
                        min_executed_len,
                        max_executed_len,
                        useful_executed: Some((min_executed_len, max_executed_len)),
                        arrivals: 0,
                        lcp_len: 0,
                    },
                )
                .collect(),
            commands: Vec::new(),
            terminal: SmrTerminal::Ran { rounds: rows.len() },
            safety_ok: true,
            exec_dup_round: None,
            retractions: 0,
            retraction_rewrites: 0,
            first_retraction_round: None,
            strong_safety_violation_rounds: 0,
            strong_safety_first_violation: None,
            strong_safety_boundary_violations: 0,
            strong_safety_boundary_first_violation: None,
            repeat_skips: 0,
            first_repeat_skip_round: None,
            boundary_skip_events: 0,
            first_boundary_skip_round: None,
            last_boundary_skip_round: None,
            max_rejoin_rounds: None,
            longest_stale_rounds: None,
            unrejoined_nodes: 0,
            horizon_skip_nodes: 0,
            wedged_round: None,
            failure: None,
            recovery: Some(RecoveryReport {
                fork_ok: true,
                commit_fork_round: None,
                rounds: rows
                    .iter()
                    .enumerate()
                    .map(|(i, &(_, reset, lo, hi))| RecRoundMetrics {
                        noreset: 0,
                        reset,
                        bot_r: 0,
                        window: i as u64 / 10,
                        rollbacks: 0,
                        max_checkpoint_window: 0,
                        rollback_depth: 0,
                        cp_fork_k: 0,
                        max_cp_p_len: 0,
                        useful_agree: Some(lo == hi),
                    })
                    .collect(),
            }),
            pool_peak_in_flight: None,
            spill_path: None,
        }
    }

    /// 40 rounds, T = 10: non-boundary rows are never consulted; boundary
    /// rows 10/20/30/40 get the given values.
    fn boundaries(vals: [(u32, u32, u32, u32); 4]) -> SmrReport {
        let mut rows = vec![(0, 0, 0, 0); 40];
        for (i, v) in vals.into_iter().enumerate() {
            rows[(i + 1) * 10 - 1] = v;
        }
        report(&rows)
    }

    #[test]
    fn earliest_boundary_with_a_clean_suffix_and_convergence_wins() {
        // Boundary 10 still reset; 20 onward clean (nonbot ≥ ceil(0.75·32)
        // = 24) and converged at 20.
        let r = boundaries([(0, 32, 0, 0), (32, 0, 1, 1), (32, 0, 1, 1), (32, 0, 1, 1)]);
        assert_eq!(r.recovered_round(32, 10), Some(20));
    }

    #[test]
    fn a_late_relapse_disqualifies_every_earlier_boundary() {
        // Clean and converged from 10, but boundary 40 relapses into reset:
        // the "every boundary ≥ r*" universal fails for all candidates.
        let r = boundaries([(32, 0, 1, 1), (32, 0, 1, 1), (32, 0, 1, 1), (32, 5, 1, 1)]);
        assert_eq!(r.recovered_round(32, 10), None);
    }

    #[test]
    fn recovery_exactly_at_the_horizon_boundary_counts() {
        // Only the last boundary qualifies; nonbot exactly at the ceil(3n/4)
        // floor passes.
        let r = boundaries([(0, 32, 0, 0), (0, 32, 0, 0), (0, 32, 0, 0), (24, 0, 2, 2)]);
        assert_eq!(r.recovered_round(32, 10), Some(40));
    }

    #[test]
    fn convergence_is_required_at_r_star_itself() {
        // Boundaries 20+ are clean but executed lens still differ at 20;
        // converged from 30.
        let r = boundaries([(0, 32, 0, 0), (32, 0, 1, 2), (32, 0, 2, 2), (32, 0, 2, 2)]);
        assert_eq!(r.recovered_round(32, 10), Some(30));
    }

    #[test]
    fn below_floor_nonbot_pushes_r_star_past_the_dip() {
        // 23 < ceil(0.75·32) = 24 at boundary 30 disqualifies r* ∈ {10, 20,
        // 30}; the suffix from 40 is clean, so the run recovered at 40.
        let r = boundaries([(0, 32, 0, 0), (32, 0, 1, 1), (23, 0, 1, 1), (32, 0, 1, 1)]);
        assert_eq!(r.recovered_round(32, 10), Some(40));
    }

    #[test]
    fn a_lagging_blocked_server_does_not_hold_back_convergence_among_useful_servers() {
        // Lengths over all servers differ at every boundary; the useful servers agree from 20.
        let mut r = boundaries([(0, 32, 0, 0), (32, 0, 1, 2), (32, 0, 1, 2), (32, 0, 1, 2)]);
        for b in [20, 30, 40] {
            r.recovery.as_mut().unwrap().rounds[b - 1].useful_agree = Some(true);
        }
        assert_eq!(r.recovered_round(32, 10), Some(20));
    }

    #[test]
    fn a_lagging_useful_server_still_blocks_convergence() {
        let mut r = boundaries([(0, 32, 0, 0), (32, 0, 2, 2), (32, 0, 2, 2), (32, 0, 2, 2)]);
        r.recovery.as_mut().unwrap().rounds[19].useful_agree = Some(false);
        assert_eq!(r.recovered_round(32, 10), Some(30));
    }

    #[test]
    fn equal_lengths_with_different_sequences_are_not_convergence() {
        let mut r = boundaries([(0, 32, 0, 0), (32, 0, 2, 2), (32, 0, 2, 2), (32, 0, 2, 2)]);
        r.recovery.as_mut().unwrap().rounds[19].useful_agree = Some(false);
        assert_eq!(r.metrics[19].useful_executed, Some((2, 2)));
        assert_eq!(r.recovered_round(32, 10), Some(30));
    }

    #[test]
    fn no_useful_server_is_not_convergence() {
        let mut r = boundaries([(32, 0, 1, 1), (32, 0, 1, 1), (32, 0, 1, 1), (32, 0, 1, 1)]);
        r.recovery.as_mut().unwrap().rounds[9].useful_agree = None;
        assert_eq!(r.recovered_round(32, 10), Some(20));
    }

    #[test]
    fn non_recovery_reports_have_no_recovered_round() {
        let mut r = report(&[(32, 0, 1, 1); 40]);
        r.recovery = None;
        assert_eq!(r.recovered_round(32, 10), None);
    }
}

// --- per-node R state: recovery-only, aggregate-consistent ---

#[test]
fn node_glances_carry_r_only_under_recovery() {
    // Per-node R is not derivable from log_len — Lemma 6.2 is one-way, so a
    // Reset node may still hold a log. Extended and compact have no R at all,
    // and their step payloads must not grow a key for it.
    for proto in [Proto::Extended, Proto::Compact { t_commit_rounds: 6 }] {
        let mut st = sim::smr::SmrState::new(
            16,
            Config::default(),
            proto,
            5.0,
            3,
            &[inj(2, 1, 7)],
            &[],
            ClientModel::Unique,
            None,
        );
        for _ in 0..8 {
            let status = st.step_masked(&[false; 16]);
            let json = serde_json::to_string(&status).expect("status serializes");
            assert!(
                !json.contains("\"r\":"),
                "{proto:?}: glances must not carry an R key"
            );
            assert!(
                !json.contains("\"rec\""),
                "{proto:?}: step payloads must not carry a rec block"
            );
            assert!(status.nodes.iter().all(|g| g.r.is_none()));
            assert!(status.rec.is_none());
        }
    }

    let s = re_aged_resend_run();
    let mut st = sim::smr::SmrState::new(
        s.n,
        s.cfg,
        s.proto,
        s.sigma,
        s.seed,
        &s.injections,
        &[],
        ClientModel::Unique,
        None,
    );
    for round in 1..=s.max_rounds {
        let mask = vec![(13..=22).contains(&round); s.n];
        let status = st.step_masked(&mask);
        let mut counts = [0u32; 3];
        for g in &status.nodes {
            counts[g.r.expect("recovery glance carries R") as usize] += 1;
        }
        let row = st.report().recovery.expect("recovery block").rounds[round - 1];
        assert_eq!(
            counts,
            [row.noreset, row.reset, row.bot_r],
            "round {round}: glance R must sum to the row's composition"
        );
    }
}

// --- display edges: adoption and reset-vote provenance ---

#[test]
fn adoption_and_reset_vote_edges_mirror_the_state_they_explain() {
    // A lone straggler blocked 100–150 comes back mid-window. There its state
    // can move ONLY by checkpoint adoption and its R can leave ⊥ only by
    // seeing a no-reset reply, so both effects must carry an edge — and every
    // edge must be backed by the state change it claims to explain.
    const N: usize = 64;
    const T: usize = 40;
    let mut st = sim::smr::SmrState::new(
        N,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: T as u64,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        5.0,
        3,
        &[inj(5, 1, 7)],
        &[],
        ClientModel::Unique,
        None,
    );
    let mut prev: Option<(Vec<u8>, Vec<u32>)> = None;
    let mut straggler_adopted: Vec<usize> = Vec::new();
    let mut straggler_voted: Vec<usize> = Vec::new();

    for round in 1..=170usize {
        let mut mask = vec![false; N];
        mask[0] = (100..=150).contains(&round);
        let status = st.step_masked(&mask);
        let rec = status.rec.clone().expect("recovery steps carry edges");
        let r_now: Vec<u8> = status.nodes.iter().map(|g| g.r.expect("R")).collect();
        let ex_now: Vec<u32> = status.nodes.iter().map(|g| g.executed_len).collect();

        let mut targets: Vec<u32> = rec.adoptions.iter().map(|&(_, to)| to).collect();
        let fired = targets.len();
        targets.sort_unstable();
        targets.dedup();
        assert_eq!(targets.len(), fired, "round {round}: one adoption per node");
        for &(from, to) in rec.adoptions.iter().chain(&rec.reset_votes) {
            assert_ne!(from, to, "round {round}: a node never explains itself");
        }
        for &(_, to) in &rec.reset_votes {
            assert_eq!(
                r_now[to as usize], 0,
                "round {round}: a vote target ends the round no-reset"
            );
        }

        // The previous status is pre-boundary, so only compare across a gap
        // that no between-window pass crossed.
        if let Some((r_prev, ex_prev)) = &prev
            && (round - 1) % T != 0
        {
            for i in 0..N {
                if r_now[i] == 0 && r_prev[i] != 0 {
                    assert!(
                        rec.reset_votes.iter().any(|&(_, to)| to == i as u32),
                        "round {round}: node {i} flipped to no-reset with no vote edge"
                    );
                }
                if ex_now[i] != ex_prev[i] {
                    assert!(
                        rec.adoptions.iter().any(|&(_, to)| to == i as u32),
                        "round {round}: node {i} moved state mid-window without adopting"
                    );
                }
            }
        }
        if rec.adoptions.iter().any(|&(_, to)| to == 0) {
            straggler_adopted.push(round);
        }
        if rec.reset_votes.iter().any(|&(_, to)| to == 0) {
            straggler_voted.push(round);
        }
        prev = Some((r_now, ex_now));
    }

    assert_eq!(
        straggler_voted,
        vec![151],
        "R leaves ⊥ exactly in the release round, and only the transition fires"
    );
    assert!(
        straggler_adopted.iter().any(|&r| (151..160).contains(&r)),
        "the straggler catches up by adoption, got {straggler_adopted:?}"
    );
}

// --- on-demand node detail: the drilldown surface ---

#[test]
fn node_detail_reports_the_quiet_run_state_it_should() {
    // A run with no traffic: every log stays the genesis no-op, nothing is
    // ever executed, so both display hashes are the FNV offset basis and the
    // aged prefix is the no-op itself once boundary 10 has passed.
    let mut st = sim::smr::SmrState::new(
        8,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: 10,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        5.0,
        9,
        &[],
        &[],
        ClientModel::Unique,
        None,
    );
    const EMPTY_HASH: &str = "cbf29ce484222325";

    for _ in 1..=5 {
        st.step_masked(&[false; 8]);
    }
    let early = st.rec_node_detail(0).expect("recovery detail");
    assert_eq!(
        (early.checkpoint_window, early.checkpoint_p_len),
        (0, None),
        "the genesis checkpoint carries P = ⊥, which is not an empty prefix"
    );

    for _ in 6..=15 {
        st.step_masked(&[false; 8]);
    }
    let d = st.rec_node_detail(0).expect("recovery detail");
    assert_eq!(d.node, 0);
    assert_eq!(d.r, 0, "no-reset on a quiet run");
    assert_eq!(d.log_len, Some(1));
    assert_eq!(d.executed_len, 0);
    assert_eq!(d.checkpoint_window, 1);
    assert_eq!(d.checkpoint_p_len, Some(1), "the aged genesis no-op");
    assert_eq!(d.s_hash, EMPTY_HASH, "nothing executed hashes to the basis");
    assert_eq!(d.checkpoint_s_hash, EMPTY_HASH);
    assert_eq!(st.rec_node_detail(8), None, "node id out of range");
}

#[test]
fn display_hashes_separate_exactly_the_divergent_histories() {
    // The hash is a display shorthand for "same executed sequence" — it must
    // agree with the sequences themselves on every pair, all run long.
    let s = re_aged_resend_run();
    let mut st = sim::smr::SmrState::new(
        s.n,
        s.cfg,
        s.proto,
        s.sigma,
        s.seed,
        &s.injections,
        &[],
        ClientModel::Unique,
        None,
    );
    for round in 1..=s.max_rounds {
        let mask = vec![(13..=22).contains(&round); s.n];
        st.step_masked(&mask);
        let seqs = st.executed_seqs();
        let hashes: Vec<String> = (0..s.n)
            .map(|i| st.rec_node_detail(i).expect("detail").s_hash)
            .collect();
        for i in 0..s.n {
            for j in (i + 1)..s.n {
                assert_eq!(
                    hashes[i] == hashes[j],
                    seqs[i] == seqs[j],
                    "round {round}: nodes {i}/{j} hash and history disagree"
                );
            }
        }
    }
}

#[test]
fn node_detail_is_recovery_only() {
    for proto in [Proto::Extended, Proto::Compact { t_commit_rounds: 6 }] {
        let mut st = sim::smr::SmrState::new(
            8,
            Config::default(),
            proto,
            5.0,
            3,
            &[inj(2, 1, 7)],
            &[],
            ClientModel::Unique,
            None,
        );
        st.step_masked(&[false; 8]);
        assert_eq!(
            st.rec_node_detail(0),
            None,
            "{proto:?} has no R or checkpoint"
        );
    }
}

// --- byte-identity: recovery fields never serialize for other protos ---

#[test]
fn ext_and_comp_reports_serialize_without_recovery_fields() {
    let ext = run_smr(&SmrScenario {
        n: 16,
        seed: 3,
        cfg: Config::default(),
        sigma: 5.0,
        proto: Proto::Extended,
        injections: vec![inj(2, 1, 7)],
        max_rounds: 20,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: None,
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    });
    let comp = run_smr(&SmrScenario {
        n: 16,
        seed: 3,
        cfg: Config::default(),
        sigma: 5.0,
        proto: Proto::Compact { t_commit_rounds: 6 },
        injections: vec![inj(2, 1, 7)],
        max_rounds: 20,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: None,
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    });
    for report in [&ext, &comp] {
        let json = serde_json::to_string(report).unwrap();
        assert!(!json.contains("\"recovery\""), "skip_serializing_if armed");
        assert!(!json.contains("\"executed_round\""));
        let cmd_json = serde_json::to_string(&report.commands[0]).unwrap();
        assert!(!cmd_json.contains("\"executed_round\""));
    }
    assert!(ext.recovery.is_none());
    assert!(comp.recovery.is_none());
}

// --- manual block overlay: the lab's targeted-blocking affordance ---

fn pinned(round: usize, node: u32) -> Injection {
    Injection {
        round,
        client: 1,
        op: 7,
        target: Some(node),
    }
}

fn attempt_at(report: &sim::smr::SmrReport, round: usize) -> Option<sim::smr::AttemptOutcome> {
    report.commands[0]
        .attempts
        .iter()
        .find(|a| a.round == round)
        .map(|a| a.outcome)
}

#[test]
fn a_manual_block_masks_exactly_its_node_over_a_half_open_window() {
    // A client pinned to node 3 makes the mask observable per node: it is
    // told TargetBlocked exactly while the overlay holds.
    let mut s = scenario(16, 7, 10, vec![pinned(5, 3)]);
    s.max_rounds = 20;
    let plain = run_smr(&s);
    s.manual_blocks = vec![sim::smr::ManualBlock {
        node: 3,
        from_round: 5,
        to_round: Some(8),
    }];
    let overlaid = run_smr(&s);

    assert_eq!(
        plain.metrics[..4],
        overlaid.metrics[..4],
        "the overlay draws nothing: rounds before it are identical"
    );
    for round in 5..8 {
        assert_eq!(
            attempt_at(&overlaid, round),
            Some(sim::smr::AttemptOutcome::TargetBlocked),
            "round {round}"
        );
    }
    // The unblocked twin reaches node 3 every round; Alg 5 amplifies while x ∉ L_3, then ignores it.
    let delivered = Some(sim::smr::AttemptOutcome::Delivered);
    let ignored = Some(sim::smr::AttemptOutcome::Ignored);
    assert_eq!(
        [5, 6, 7].map(|round| attempt_at(&plain, round)),
        [delivered, delivered, ignored],
        "unblocked twin"
    );
    // Half-open: round 8 is off the mask. The node still answers ⊥ because
    // three blocked rounds emptied its log — that is the block's aftermath,
    // not the block, and only an unmasked target can report it.
    assert_eq!(
        attempt_at(&overlaid, 8),
        Some(sim::smr::AttemptOutcome::TargetBot),
        "the window is half-open — round 8 is off the mask"
    );
    for (i, m) in overlaid.metrics.iter().enumerate() {
        assert_eq!(
            m.blocked,
            u32::from((5..8).contains(&(i + 1))),
            "round {}",
            i + 1
        );
    }
}

#[test]
fn an_open_ended_manual_block_runs_to_the_horizon_under_every_schedule() {
    let schedules = [
        BlockSchedule::FreshPerRound { fraction: 0.1 },
        BlockSchedule::Permanent { fraction: 0.1 },
        BlockSchedule::PerRoundFractions(vec![0.0, 0.2, 0.1]),
        BlockSchedule::PerRoundSticky {
            background: 0.05,
            targets: vec![0.1, 0.2],
        },
        BlockSchedule::Windows(vec![window(4, 3, BlockTarget::SampleFraction(0.2))]),
    ];
    for schedule in schedules {
        let mut s = scenario(16, 7, 10, vec![pinned(2, 3)]);
        s.max_rounds = 15;
        s.schedule = schedule.clone();
        s.manual_blocks = vec![sim::smr::ManualBlock {
            node: 3,
            from_round: 2,
            to_round: None,
        }];
        let report = run_smr(&s);
        for round in 2..=15 {
            assert_eq!(
                attempt_at(&report, round),
                Some(sim::smr::AttemptOutcome::TargetBlocked),
                "{schedule:?} round {round}"
            );
        }
        assert!(
            report.metrics[1..].iter().all(|m| m.blocked >= 1),
            "{schedule:?}: the overlay unions onto whatever the schedule drew"
        );
    }
}

#[test]
fn manual_blocks_stay_off_the_wire_when_unused() {
    let json = r#"{"n":16,"seed":7,"k":6,"ell":3,"sigma":5.0,
        "proto":{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false},
        "injections":[],"max_rounds":25}"#;
    let spec: SmrScenarioSpec =
        serde_json::from_str(json).expect("a spec without an overlay parses");
    assert!(spec.manual_blocks.is_empty());
    let round_tripped = serde_json::to_string(&spec).expect("serializes");
    assert!(
        !round_tripped.contains("manual_blocks"),
        "an empty overlay must not appear in exported specs"
    );

    let with = r#"{"n":16,"seed":7,"k":6,"ell":3,"sigma":5.0,
        "proto":{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false},
        "injections":[],"max_rounds":25,
        "manual_blocks":[{"node":3,"from_round":5},{"node":4,"from_round":2,"to_round":9}]}"#;
    let spec: SmrScenarioSpec = serde_json::from_str(with).expect("overlay parses");
    let scenario = SmrScenario::try_from(spec).expect("valid");
    assert_eq!(scenario.manual_blocks.len(), 2);
    assert_eq!(scenario.manual_blocks[0].to_round, None);
    assert_eq!(scenario.manual_blocks[1].to_round, Some(9));
}

#[test]
fn manual_block_validation_rejects_impossible_windows() {
    let cases = [
        (r#"{"node":16,"from_round":1}"#, "out of range"),
        (r#"{"node":0,"from_round":0}"#, "1-based"),
        (r#"{"node":0,"from_round":5,"to_round":5}"#, "after"),
    ];
    for (block, needle) in cases {
        let json = format!(
            r#"{{"n":16,"seed":7,"k":6,"ell":3,"sigma":5.0,
            "proto":{{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}},
            "injections":[],"max_rounds":25,"manual_blocks":[{block}]}}"#
        );
        let spec: SmrScenarioSpec = serde_json::from_str(&json).expect("parses");
        let err = SmrScenario::try_from(spec).expect_err("must be rejected");
        assert!(err.contains(needle), "got: {err}");
    }
}

// --- wire form (sweep-grid axis protocol: recovery via base.proto) ---

#[test]
fn recovery_spec_wire_form_round_trips_and_rejects_zero_window() {
    let json = r#"{"n":16,"seed":7,"k":6,"ell":3,"sigma":5.0,
        "proto":{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false},
        "injections":[],"max_rounds":25}"#;
    let spec: SmrScenarioSpec = serde_json::from_str(json).unwrap();
    let scenario = SmrScenario::try_from(spec).unwrap();
    assert_eq!(
        scenario.proto,
        Proto::Recovery {
            t_window_rounds: 10,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        }
    );
    let report = run_smr(&scenario);
    assert!(matches!(report.terminal, SmrTerminal::Ran { rounds: 25 }));

    let bad = r#"{"n":16,"seed":7,"k":6,"ell":3,"sigma":5.0,
        "proto":{"kind":"recovery","t_window_rounds":0,"resend_until_acked":false},
        "injections":[],"max_rounds":25}"#;
    let bad_spec: SmrScenarioSpec = serde_json::from_str(bad).unwrap();
    let err = SmrScenario::try_from(bad_spec).unwrap_err();
    assert!(err.contains("t_window_rounds"), "got: {err}");

    // The tagged enum stays additive: existing proto forms parse unchanged.
    let ext = r#"{"n":16,"seed":7,"k":6,"ell":3,"sigma":5.0,
        "proto":{"kind":"extended"},"injections":[],"max_rounds":25}"#;
    let ext_spec: SmrScenarioSpec = serde_json::from_str(ext).unwrap();
    assert_eq!(
        SmrScenario::try_from(ext_spec).unwrap().proto,
        Proto::Extended
    );
    let comp = r#"{"n":16,"seed":7,"k":6,"ell":3,"sigma":5.0,
        "proto":{"kind":"compact","t_commit_rounds":6},"injections":[],"max_rounds":25}"#;
    let comp_spec: SmrScenarioSpec = serde_json::from_str(comp).unwrap();
    assert_eq!(
        SmrScenario::try_from(comp_spec).unwrap().proto,
        Proto::Compact { t_commit_rounds: 6 }
    );
}

// --- canonical committed sequence (the lab chain panel's source) ---

/// A hand-stepped recovery session under load, so boundaries have a backlog
/// to commit and the sequence actually grows. σ matches `scenario` above:
/// under load, thin amplification lets logs diverge faster than a T-window
/// reconciles them, and the boundary strip aborts on a non-extending P.
fn rec_state(n: usize, seed: u64, t_window: u64) -> SmrState {
    SmrState::new(
        n,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: t_window,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        5.0,
        seed,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.4, 0.6],
        }],
        ClientModel::Unique,
        None,
    )
}

#[test]
fn committed_entries_extends_every_nodes_executed_prefix_and_grows_only_at_boundaries() {
    const T: u64 = 10;
    const N: usize = 32;
    let mut state = rec_state(N, 11, T);
    let no_block = vec![false; N];
    let mut prev: Vec<Entry> = Vec::new();
    let mut grew_at: Vec<u64> = Vec::new();
    for round in 1..=60u64 {
        state.draw_arrivals();
        state.step_masked(&no_block);
        let committed = state
            .committed_entries()
            .expect("recovery exposes a sequence");
        // The canonicality claim the forest rests on: every server's executed
        // sequence is a prefix of it, so one MMR covers the whole population.
        for node in 0..N {
            let executed = state.executed_entries(node).expect("recovery executed seq");
            assert!(
                committed.iter().take(executed.len()).eq(executed.iter()),
                "round {round}: node {node} is not a prefix of the canonical sequence"
            );
        }
        assert!(
            committed.len() >= prev.len() && committed[..prev.len()] == prev[..],
            "round {round}: canonical sequence regressed"
        );
        if committed.len() > prev.len() {
            grew_at.push(round);
            prev = committed.to_vec();
        }
    }
    assert!(!prev.is_empty(), "the run must commit something");
    for round in &grew_at {
        assert_eq!(round % T, 0, "committed mid-window at round {round}");
    }
    assert!(
        grew_at.len() >= 3,
        "expected several commit boundaries, got {grew_at:?}"
    );
}

#[test]
fn committed_entries_never_shrinks_through_an_all_bot_blackout() {
    // What makes an MMR over the sequence well-defined at all times: rollback
    // restores checkpoints but never un-executes (Lemma 6.9), so the sequence
    // is monotone even across a stretch where no server holds a log at all —
    // the case that rules out reading it off non-⊥ servers only.
    const T: u64 = 10;
    const N: usize = 32;
    let mut state = rec_state(N, 11, T);
    let no_block = vec![false; N];
    let all_block = vec![true; N];
    let mut prev: Vec<Entry> = Vec::new();
    let mut before_blackout = 0;
    let mut all_bot_rounds = 0;
    for round in 1..=90u64 {
        state.draw_arrivals();
        let blackout = (35..=62).contains(&round);
        state.step_masked(if blackout { &all_block } else { &no_block });
        let committed = state
            .committed_entries()
            .expect("recovery exposes a sequence");
        assert!(
            committed.len() >= prev.len() && committed[..prev.len()] == prev[..],
            "round {round}: canonical sequence regressed from {} to {}",
            prev.len(),
            committed.len()
        );
        prev = committed.to_vec();
        if round == 34 {
            before_blackout = committed.len();
        }
        let all_bot = (0..N).all(|i| {
            state
                .rec_node_detail(i)
                .expect("recovery node detail")
                .log_len
                .is_none()
        });
        if all_bot {
            all_bot_rounds += 1;
            assert!(
                committed.len() >= before_blackout,
                "round {round}: the sequence vanished with the last log"
            );
        }
    }
    assert!(before_blackout > 0, "commits must land before the blackout");
    assert!(
        all_bot_rounds >= 10,
        "the blackout must drive the whole population to ⊥, saw {all_bot_rounds} rounds"
    );
    assert!(
        prev.len() > before_blackout,
        "the run must recover and commit again"
    );
}

#[test]
fn committed_entries_is_recovery_only() {
    for proto in [Proto::Extended, Proto::Compact { t_commit_rounds: 6 }] {
        let mut state = SmrState::new(
            16,
            Config::default(),
            proto,
            1.5,
            3,
            &[inj(2, 1, 7)],
            &[],
            ClientModel::Unique,
            None,
        );
        let no_block = vec![false; 16];
        for _ in 0..20 {
            state.step_masked(&no_block);
        }
        assert!(
            state.committed_entries().is_none(),
            "{proto:?} has no §6 committed sequence to expose"
        );
    }
}

/// The boundary pass answers executed-membership over POST-boundary shared
/// states: `end_window` commits P inside the same pass, so a command committed
/// at this boundary has to latch `executed_round` now and not a window later.
/// The reference is the membership scan itself, run from outside on exactly
/// the state each boundary left behind.
#[test]
fn the_boundary_latch_fires_the_round_the_commit_lands() {
    let n = 32;
    let t_window = 20;
    let mut st = SmrState::new(
        n,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: t_window,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        5.0,
        4242,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: sim::smr::point_mass_pmf(2),
        }],
        ClientModel::Unique,
        None,
    );
    // op → the first boundary round at which every useful server had it.
    let mut expected: std::collections::BTreeMap<u64, usize> = std::collections::BTreeMap::new();
    for round in 1..=160usize {
        // Warm up clean, spiral the population so boundaries roll back, release.
        let fraction = if (60..=100).contains(&round) {
            0.6
        } else {
            0.05
        };
        st.step_fraction(fraction);
        if !round.is_multiple_of(t_window as usize) {
            continue;
        }
        let seqs = st.executed_seqs();
        let useful: Vec<bool> = (0..n)
            .map(|i| st.rec_node_detail(i).expect("detail").log_len.is_some())
            .collect();
        if !useful.iter().any(|&u| u) {
            continue;
        }
        let longest = seqs.iter().max_by_key(|s| s.len()).expect("a population");
        for entry in longest {
            let Entry::Cmd(cc) = entry else { continue };
            if seqs
                .iter()
                .zip(&useful)
                .all(|(seq, &u)| !u || seq.contains(entry))
            {
                expected.entry(cc.op).or_insert(round);
            }
        }
    }
    let report = st.report();
    let rows = &report.recovery.as_ref().expect("recovery block").rounds;
    assert!(
        rows.iter().map(|r| r.rollbacks).sum::<u32>() > 0,
        "the surge must roll checkpoints back"
    );
    assert!(
        expected.len() > 10,
        "only {} commands committed — too few to pin the latch",
        expected.len()
    );
    for c in &report.commands {
        assert_eq!(
            c.executed_round,
            expected.get(&c.op).copied(),
            "op {}: the latch disagrees with the membership scan",
            c.op
        );
    }
}
