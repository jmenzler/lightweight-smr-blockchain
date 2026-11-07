use sim::smr::{
    ClientModel, Injection, PrefixMismatch, Proto, RepeatedCommit, SmrScenario, SmrState,
    amp_count, auto_command,
};
use sim::{BlockSchedule, Config};

fn scenario(injections: Vec<Injection>) -> SmrScenario {
    SmrScenario {
        n: 16,
        seed: 7,
        cfg: Config::default(),
        sigma: 5.0,
        proto: Proto::Extended,
        injections,
        max_rounds: 50,
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
fn amp_count_is_ceil_sigma_log2_n() {
    assert_eq!(amp_count(5.0, 1000), 50); // 5 · 9.9658… → 50
    assert_eq!(amp_count(1.0, 1024), 10); // exact power of two
    assert_eq!(amp_count(2.0, 2), 2);
    assert_eq!(
        amp_count(1.0, 1),
        1,
        "log₂ 1 = 0 still sends one append request"
    );
    assert_eq!(
        amp_count(16.6, 32768),
        249,
        "no float noise above an exact integer"
    );
    assert_eq!(amp_count(0.7, 1024), 7);
}

#[test]
fn sigma_must_meet_alg_3_and_bound_the_append_fan_out() {
    let with_sigma = |sigma: f64| {
        let mut s = scenario(vec![]);
        s.sigma = sigma;
        s.validate()
    };
    assert!(with_sigma(1.0).is_ok());
    assert!(with_sigma(0.5).is_err(), "Alg 3 B1 requires σ ≥ 1");
    assert!(with_sigma(f64::NAN).is_err());
    assert!(
        with_sigma(1e30).is_err(),
        "a saturating fan-out would never finish"
    );
}

#[test]
fn a_compact_commit_window_of_zero_rounds_is_rejected() {
    let mut s = scenario(vec![]);
    s.proto = Proto::Compact { t_commit_rounds: 0 };
    assert!(s.validate().is_err());
    s.proto = Proto::Compact { t_commit_rounds: 1 };
    assert!(s.validate().is_ok());
}

#[test]
fn a_recovery_window_of_zero_rounds_is_rejected() {
    let mut s = scenario(vec![]);
    let recovery = |t_window_rounds| Proto::Recovery {
        t_window_rounds,
        resend_until_acked: true,
        prefix_mismatch: PrefixMismatch::Abort,
    };
    s.proto = recovery(0);
    let err = s.validate().expect_err("a recovery window of 0 rounds");
    assert!(err.contains("t_window_rounds"), "{err}");
    s.proto = recovery(1);
    assert!(s.validate().is_ok());
}

#[test]
#[should_panic(expected = "auto client overflow")]
fn the_auto_client_namespace_does_not_wrap() {
    let _ = auto_command(1 << 31);
}

#[test]
fn scenario_validation_rejects_bad_injections() {
    assert!(scenario(vec![inj(1, 1, 1)]).validate().is_ok());
    assert!(scenario(vec![]).validate().is_ok(), "no injections is fine");

    let mut s = scenario(vec![inj(1, 1, 1)]);
    s.sigma = 0.0;
    assert!(s.validate().is_err(), "sigma must be positive");

    assert!(
        scenario(vec![inj(0, 1, 1)]).validate().is_err(),
        "rounds are 1-based"
    );
    assert!(
        scenario(vec![inj(1, 1, 0)]).validate().is_err(),
        "op 0 is the reserved seed command"
    );
    assert!(
        scenario(vec![inj(1, 1, 7), inj(2, 2, 7)])
            .validate()
            .is_err(),
        "ops must be distinct"
    );
    assert!(
        scenario(vec![inj(1, 1, 7), inj(2, 1, 8)])
            .validate()
            .is_ok(),
        "a client may inject repeatedly: its b-th command carries sn = b"
    );
}

#[test]
fn repeated_commit_skip_needs_a_compact_or_certless_recovery_node() {
    let mut s = scenario(vec![]);
    s.repeated_commit = RepeatedCommit::Skip;
    assert!(s.validate().is_err(), "extended nodes keep no sn(c)");

    s.proto = Proto::Compact { t_commit_rounds: 8 };
    assert!(s.validate().is_ok());

    s.proto = Proto::Recovery {
        t_window_rounds: 8,
        resend_until_acked: false,
        prefix_mismatch: PrefixMismatch::Abort,
    };
    assert!(s.validate().is_ok());
    s.certs = true;
    assert!(s.validate().is_err(), "§5 would attest a skipped command");
}

#[test]
fn scenario_validation_enforces_the_median_rule_preconditions() {
    for (k, ell) in [(6, 0), (6, 1), (6, 4), (3, 5), (1, 1)] {
        let mut s = scenario(vec![]);
        s.cfg = Config { k, ell };
        let err = s.validate().expect_err("forged (k, ell) must be rejected");
        assert!(
            err.contains("k, ell"),
            "unhelpful message for ({k},{ell}): {err}"
        );
    }
    let mut s = scenario(vec![]);
    s.cfg = Config::new(3, 3).unwrap();
    assert!(s.validate().is_ok());
}

fn live_state(injections: &[Injection]) -> SmrState {
    SmrState::new(
        16,
        Config::default(),
        Proto::Compact { t_commit_rounds: 6 },
        1.0,
        3,
        injections,
        &[],
        ClientModel::Unique,
        None,
    )
}

#[test]
fn a_live_injection_cannot_jump_ahead_of_a_scheduled_command_of_its_client() {
    let mut state = live_state(&[inj(50, 5, 10)]);
    assert!(
        state.inject(5, 11, None).is_err(),
        "replay numbers by round, so op 11 would be sn 1 there and sn 2 here"
    );
    assert!(
        state.inject(6, 11, None).is_ok(),
        "another client is unaffected"
    );

    let mut state = live_state(&[inj(1, 5, 10)]);
    assert_eq!(state.inject(5, 11, None), Ok(1));
    let replay = sim::smr::injection_sns(&state.injections());
    assert_eq!(
        replay,
        vec![1, 2],
        "the live numbering is the replay numbering"
    );
}
