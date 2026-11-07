use sim::{AdaptivePolicy, BlockSchedule, Config, Init, Scenario, run_traced};

fn scenario(n: usize, seed: u64, fraction: f64, max_rounds: usize) -> Scenario {
    Scenario {
        n,
        seed,
        cfg: Config::new(6, 3).unwrap(),
        init: Init::WithUndecided {
            useful_fraction: 0.5,
            inner: Box::new(Init::Split { fraction: 0.5 }),
        },
        max_rounds,
        schedule: BlockSchedule::Adaptive0Late {
            fraction,
            policy: AdaptivePolicy::BlockHolders,
        },
        partition: None,
    }
}

/// The block_holders policy invariant of the 0-late schedule: from round 2 on, the
/// budget lands on the holders at the beginning of round t first — a uniform sample
/// of holders when they cover the budget, all holders plus a non-holder remainder
/// when they don't.
#[test]
fn adaptive_blocks_holders_first_after_round_one() {
    let (_, trace) = run_traced(&scenario(40, 7, 0.1, 6));
    let budget = 4;
    let mut checked = 0;
    for t in 1..trace.rounds.len() {
        let prev = &trace.rounds[t - 1].states;
        let holders: Vec<u32> = (0..40u32).filter(|&i| prev[i as usize].is_some()).collect();
        let blocked = &trace.rounds[t].blocked;
        assert_eq!(
            blocked.len(),
            budget,
            "round {}: budget always spent",
            t + 1
        );
        if holders.len() >= budget {
            assert!(
                blocked.iter().all(|id| holders.contains(id)),
                "round {}: blocked {:?} not a subset of prior holders",
                t + 1,
                blocked
            );
        } else {
            assert!(
                holders.iter().all(|id| blocked.contains(id)),
                "round {}: all {} prior holders must be blocked before spillover",
                t + 1,
                holders.len()
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 2,
        "need rounds beyond the first to exercise the policy"
    );
}

/// Round 1 has no prior snapshot: the registered fallback is the plain uniform
/// draw, so its blocked set must equal the fresh schedule's at the same seed.
#[test]
fn adaptive_round_one_falls_back_to_uniform() {
    let mut fresh = scenario(40, 7, 0.1, 1);
    fresh.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
    let (_, uniform) = run_traced(&fresh);
    let (_, adaptive) = run_traced(&scenario(40, 7, 0.1, 1));
    assert_eq!(adaptive.rounds[0].blocked, uniform.rounds[0].blocked);
}

#[test]
fn adaptive_same_seed_same_trace() {
    let (o1, t1) = run_traced(&scenario(60, 11, 0.12, 30));
    let (o2, t2) = run_traced(&scenario(60, 11, 0.12, 30));
    assert_eq!(o1, o2);
    assert_eq!(t1.rounds.len(), t2.rounds.len());
    for (a, b) in t1.rounds.iter().zip(t2.rounds.iter()) {
        assert_eq!(a.states, b.states);
        assert_eq!(a.blocked, b.blocked);
    }
}

/// The 0-late wire form; the former `adaptive_1late` spelling is rejected (see one_late_adversary).
#[test]
fn zero_late_schedule_parses_its_wire_form() {
    let json = r#"{"n":100,"seed":0,"k":6,"ell":3,"max_rounds":10,
        "init":{"kind":"split","fraction":0.5},
        "schedule":{"kind":"adaptive_0late","fraction":0.1,"policy":"block_holders"}}"#;
    let spec: sim::spec::ScenarioSpec = serde_json::from_str(json).expect("wire form parses");
    let scenario = Scenario::try_from(spec).unwrap();
    assert_eq!(
        scenario.schedule,
        BlockSchedule::Adaptive0Late {
            fraction: 0.1,
            policy: AdaptivePolicy::BlockHolders,
        }
    );
}

fn alpha_scenario(n: usize, seed: u64, fraction: f64, max_rounds: usize, alpha: u32) -> Scenario {
    Scenario {
        n,
        seed,
        cfg: Config::new(6, 3).unwrap(),
        init: Init::WithUndecided {
            useful_fraction: 0.5,
            inner: Box::new(Init::Split { fraction: 0.5 }),
        },
        max_rounds,
        schedule: BlockSchedule::AdaptiveAlphaLate {
            fraction,
            alpha,
            policy: AdaptivePolicy::BlockHolders,
        },
        partition: None,
    }
}

/// Definition 1.1 timing: an α-late adversary choosing round t's mask knows
/// states only up to the beginning of round t−α, i.e. the end-of-round-(t−1−α)
/// snapshot. For α = 1 that is two trace rows back.
#[test]
fn alpha_late_blocks_holders_from_the_alpha_stale_snapshot() {
    let (_, trace) = run_traced(&alpha_scenario(40, 7, 0.1, 6, 1));
    let budget = 4;
    let mut checked = 0;
    for t in 2..trace.rounds.len() {
        let snap = &trace.rounds[t - 2].states;
        let holders: Vec<u32> = (0..40u32).filter(|&i| snap[i as usize].is_some()).collect();
        let blocked = &trace.rounds[t].blocked;
        assert_eq!(
            blocked.len(),
            budget,
            "round {}: budget always spent",
            t + 1
        );
        if holders.len() >= budget {
            assert!(
                blocked.iter().all(|id| holders.contains(id)),
                "round {}: blocked {:?} not a subset of the α-stale holders",
                t + 1,
                blocked
            );
        } else {
            assert!(
                holders.iter().all(|id| blocked.contains(id)),
                "round {}: all α-stale holders must be blocked before spillover",
                t + 1
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 2,
        "need rounds beyond the fallback to exercise the timing"
    );
}

/// Rounds 1..=α+1 have no snapshot old enough; the fallback is the plain
/// uniform draw, byte-identical to the fresh schedule at the same seed.
#[test]
fn alpha_late_early_rounds_fall_back_to_uniform() {
    let mut fresh = alpha_scenario(40, 7, 0.1, 2, 1);
    fresh.schedule = BlockSchedule::FreshPerRound { fraction: 0.1 };
    let (_, uniform) = run_traced(&fresh);
    let (_, adaptive) = run_traced(&alpha_scenario(40, 7, 0.1, 2, 1));
    assert_eq!(adaptive.rounds[0].blocked, uniform.rounds[0].blocked);
    assert_eq!(adaptive.rounds[1].blocked, uniform.rounds[1].blocked);
}

#[test]
fn alpha_late_same_seed_same_trace() {
    let (o1, t1) = run_traced(&alpha_scenario(60, 11, 0.12, 30, 1));
    let (o2, t2) = run_traced(&alpha_scenario(60, 11, 0.12, 30, 1));
    assert_eq!(o1, o2);
    assert_eq!(t1.rounds.len(), t2.rounds.len());
    for (a, b) in t1.rounds.iter().zip(t2.rounds.iter()) {
        assert_eq!(a.states, b.states);
        assert_eq!(a.blocked, b.blocked);
    }
}

/// The wire form the x3 trial-3 prereg registers — `adaptive_alpha_late`.
#[test]
fn alpha_late_schedule_parses_wire_form() {
    let json = r#"{"n":100,"seed":0,"k":6,"ell":3,"max_rounds":10,
        "init":{"kind":"split","fraction":0.5},
        "schedule":{"kind":"adaptive_alpha_late","fraction":0.1,"alpha":1,"policy":"block_holders"}}"#;
    let spec: sim::spec::ScenarioSpec = serde_json::from_str(json).expect("wire form parses");
    let scenario = Scenario::try_from(spec).unwrap();
    assert_eq!(
        scenario.schedule,
        BlockSchedule::AdaptiveAlphaLate {
            fraction: 0.1,
            alpha: 1,
            policy: AdaptivePolicy::BlockHolders,
        }
    );
}

/// α = 0 is the `adaptive_0late` schedule; the alpha-late kind refuses it instead of
/// duplicating the RNG path.
#[test]
fn alpha_late_rejects_alpha_zero() {
    let json = r#"{"n":100,"seed":0,"k":6,"ell":3,"max_rounds":10,
        "init":{"kind":"split","fraction":0.5},
        "schedule":{"kind":"adaptive_alpha_late","fraction":0.1,"alpha":0,"policy":"block_holders"}}"#;
    let spec: sim::spec::ScenarioSpec =
        serde_json::from_str(json).expect("parses; rejected at conversion");
    let err = Scenario::try_from(spec).unwrap_err();
    assert!(
        err.contains("alpha"),
        "error names the offending field: {err}"
    );
}

/// Adaptive schedules need the Alg-1 loop's node-state hook; the SMR path must
/// reject them at validation instead of panicking in the mask loop.
#[test]
fn smr_path_rejects_adaptive_schedules() {
    for kind in [
        r#"{"kind":"adaptive_0late","fraction":0.1,"policy":"block_holders"}"#,
        r#"{"kind":"adaptive_alpha_late","fraction":0.1,"alpha":1,"policy":"block_holders"}"#,
    ] {
        let json = format!(
            r#"{{"n":100,"seed":0,"k":6,"ell":3,"sigma":1.0,"proto":{{"kind":"extended"}},
                "injections":[],"max_rounds":10,"schedule":{kind}}}"#
        );
        let spec: sim::spec::SmrScenarioSpec =
            serde_json::from_str(&json).expect("parses; rejected at conversion");
        let err = sim::smr::SmrScenario::try_from(spec).unwrap_err();
        assert!(err.contains("Alg-1"), "error explains the scope: {err}");
    }
}
