// Stage-f ordering observability: per-round lcp_len and per-command
// position stats (the lab's prefix ribbon + position band). RNG-free —
// existing replay pins prove the stream is untouched.
use sim::smr::{ClientModel, Injection, Proto, SmrScenario, run_smr};
use sim::{BlockSchedule, Config};

const T: u64 = 6;

fn scenario(proto: Proto, n: usize, seed: u64, injections: Vec<Injection>) -> SmrScenario {
    SmrScenario {
        n,
        seed,
        cfg: Config::default(),
        sigma: 5.0,
        proto,
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

#[test]
fn extended_lcp_is_full_log_when_quiescent() {
    let report = run_smr(&scenario(Proto::Extended, 32, 7, vec![]));
    for m in &report.metrics {
        assert_eq!(m.lcp_len, 1, "all logs are [seed] — lcp is the seed");
        assert_eq!(m.max_log_len, 1);
    }
}

#[test]
fn extended_lcp_bounded_and_converges() {
    let report = run_smr(&scenario(Proto::Extended, 32, 9, vec![inj(2, 1, 7)]));
    for m in &report.metrics {
        assert!(
            m.lcp_len <= m.max_log_len,
            "lcp {} exceeds max log {}",
            m.lcp_len,
            m.max_log_len
        );
    }
    let last = report.metrics.last().unwrap();
    assert_eq!(
        last.lcp_len, last.max_log_len,
        "beta = 0: logs converge, lcp reaches full length"
    );
    assert_eq!(last.lcp_len, 2, "seed + the injected command");
}

#[test]
fn compact_lcp_equals_min_executed() {
    let report = run_smr(&scenario(
        Proto::Compact { t_commit_rounds: T },
        32,
        9,
        vec![inj(2, 1, 7)],
    ));
    for m in &report.metrics {
        assert_eq!(
            m.lcp_len, m.min_executed_len,
            "compact: prefix-consistent executed sequences make the shortest one the lcp"
        );
    }
    assert!(
        report.metrics.last().unwrap().lcp_len >= 1,
        "command executed"
    );
}

#[test]
fn extended_position_band_collapses_at_prefix_fixed() {
    let report = run_smr(&scenario(Proto::Extended, 32, 11, vec![inj(2, 1, 7)]));
    let cmd = &report.commands[0];
    let fixed = cmd.prefix_fixed_round.expect("beta = 0 completes");

    let last = cmd.spread.points().last().expect("spread sampled");
    assert!(
        last.round >= fixed,
        "sampling must reach the T_E landmark (last {} < fixed {fixed})",
        last.round
    );

    let at_fixed = cmd
        .spread
        .points()
        .iter()
        .find(|p| p.round >= fixed)
        .expect("point at/after T_E");
    let pos = at_fixed.pos_med.expect("in every log at T_E");
    assert_eq!(at_fixed.pos_min, Some(pos), "band collapsed");
    assert_eq!(at_fixed.pos_max, Some(pos), "band collapsed");
    assert_eq!(pos, 1, "seed at 0, command fixed at 1");

    let first = cmd
        .spread
        .points()
        .first()
        .expect("sampled from injection round");
    assert!(
        first.pos_min.is_none() || first.pos_min <= first.pos_max,
        "min never exceeds max"
    );
}

#[test]
fn compact_positions_follow_executed_prefix() {
    let report = run_smr(&scenario(
        Proto::Compact { t_commit_rounds: T },
        32,
        13,
        vec![inj(2, 1, 7)],
    ));
    let cmd = &report.commands[0];
    let last = cmd.spread.points().last().expect("spread sampled");
    let pos = last.pos_med.expect("committed everywhere at the end");
    assert_eq!(last.pos_min, Some(pos), "band collapsed after commit");
    assert_eq!(last.pos_max, Some(pos), "band collapsed after commit");
    assert_eq!(pos, 0, "first executed command sits at position 0");
}

// Def 1.3, p. 6 vs Lemma 3.8, p. 24: one index in every log is agreement; one identical prefix
// through x is the stronger T_E landmark, so it is never earlier and under traffic sometimes later.
#[test]
fn extended_index_agreement_is_never_later_than_the_fixed_prefix() {
    let mut later = 0;
    for seed in [1u64, 2, 3] {
        let mut sc = scenario(Proto::Extended, 128, seed, Vec::new());
        sc.sigma = 1.0;
        sc.max_rounds = 60;
        sc.traffic = Some(vec![sim::smr::TrafficPhase {
            from_round: 1,
            arrivals_pmf: sim::smr::point_mass_pmf(4),
        }]);
        for cmd in run_smr(&sc).commands {
            if let Some(fixed) = cmd.prefix_fixed_round {
                let agreed = cmd
                    .index_agreed_round
                    .expect("a fixed prefix fixes the index");
                assert!(agreed <= fixed, "op {}: {agreed} > {fixed}", cmd.op);
                later += usize::from(agreed < fixed);
            }
        }
    }
    assert!(
        later > 0,
        "no command agreed on its index before its prefix fixed"
    );
    let single = run_smr(&scenario(Proto::Extended, 32, 11, vec![inj(2, 1, 7)]));
    let cmd = &single.commands[0];
    assert_eq!(
        cmd.index_agreed_round, cmd.prefix_fixed_round,
        "one command: the notions coincide"
    );
}
