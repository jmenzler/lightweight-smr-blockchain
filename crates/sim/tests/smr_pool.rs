//! Bounded client-pool traffic model: reuse latches, exhaustion, the wire
//! surface, and the peak-in-flight instrument.

use sim::smr::{
    AUTO_CLIENT_BASE, ClientModel, CommandReport, CommandStatus, Proto, SmrReport, SmrScenario,
    TrafficPhase, point_mass_pmf, run_smr,
};
use sim::spec::{SmrScenarioSpec, TrafficSpec};
use sim::sweep::{LadderRung, SmrGridSpec, run_smr_grid_streaming};
use sim::{BlockSchedule, Config};

const N: usize = 16;
const T_COMMIT: u64 = 8;
const T_WINDOW: u64 = 10;

/// `(from_round, rate)` pairs as point-mass phases.
fn phases(spec: &[(usize, usize)]) -> Vec<TrafficPhase> {
    spec.iter()
        .map(|&(from_round, rate)| TrafficPhase {
            from_round,
            arrivals_pmf: point_mass_pmf(rate),
        })
        .collect()
}

fn scenario(
    proto: Proto,
    clients: u32,
    traffic: &[(usize, usize)],
    max_rounds: usize,
) -> SmrScenario {
    SmrScenario {
        n: N,
        seed: 7,
        cfg: Config::default(),
        sigma: 5.0,
        proto,
        injections: vec![],
        max_rounds,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: Some(phases(traffic)),
        client_model: ClientModel::Pool { clients },
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

const COMPACT: Proto = Proto::Compact {
    t_commit_rounds: T_COMMIT,
};
const RECOVERY: Proto = Proto::Recovery {
    t_window_rounds: T_WINDOW,
    resend_until_acked: false,
    prefix_mismatch: sim::smr::PrefixMismatch::Abort,
};

/// One arrival in round 1 and nothing after — the run whose latch round the
/// reuse tests below aim at.
fn single_arrival(proto: Proto) -> SmrReport {
    run_smr(&scenario(proto, 1, &[(1, 1), (2, 0)], 120))
}

fn panic_message(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let payload = std::panic::catch_unwind(f).expect_err("must panic");
    std::panic::set_hook(previous);
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .expect("string payload")
}

/// The successor of `client` in report order, i.e. the pool slot's next command.
fn successor(report: &SmrReport, client: u32) -> &CommandReport {
    let mut same: Vec<&CommandReport> = report
        .commands
        .iter()
        .filter(|c| c.client == client)
        .collect();
    same.sort_by_key(|c| c.injection_round);
    assert_eq!(same.len(), 2, "expected exactly one successor");
    same[1]
}

#[test]
fn compact_frees_a_pool_client_at_its_committed_ack_and_not_before() {
    let acked = single_arrival(COMPACT).commands[0]
        .committed_ack_round
        .expect("the command commits well inside the horizon");

    // Stage (a0) precedes the client stage, so an arrival in the ack round
    // itself still finds the slot busy.
    let same_round = panic_message(|| {
        run_smr(&scenario(
            COMPACT,
            1,
            &[(1, 1), (2, 0), (acked, 1), (acked + 1, 0)],
            120,
        ));
    });
    assert!(
        same_round.contains("client pool exhausted"),
        "an arrival in the ack round must find the slot busy, got: {same_round}"
    );

    let report = run_smr(&scenario(
        COMPACT,
        1,
        &[(1, 1), (2, 0), (acked + 1, 1), (acked + 2, 0)],
        120,
    ));
    let next = successor(&report, AUTO_CLIENT_BASE);
    assert_eq!(
        next.injection_round,
        acked + 1,
        "drawable the round after the ack"
    );
    assert!(
        next.delivered_round.is_some(),
        "the successor must be accepted for spreading, which only happens at \
         sn(c) + 1 — an unadvanced sequence number would be acked on sight"
    );
    assert_eq!(report.pool_peak_in_flight, Some(1));
}

#[test]
fn recovery_frees_a_pool_client_at_the_all_useful_executed_latch() {
    let executed = single_arrival(RECOVERY).commands[0]
        .executed_round
        .expect("the command executes well inside the horizon");

    let same_round = panic_message(|| {
        run_smr(&scenario(
            RECOVERY,
            1,
            &[(1, 1), (2, 0), (executed, 1), (executed + 1, 0)],
            120,
        ));
    });
    assert!(
        same_round.contains("client pool exhausted"),
        "an arrival in the latch round must find the slot busy, got: {same_round}"
    );

    let report = run_smr(&scenario(
        RECOVERY,
        1,
        &[(1, 1), (2, 0), (executed + 1, 1), (executed + 2, 0)],
        120,
    ));
    let next = successor(&report, AUTO_CLIENT_BASE);
    assert_eq!(next.injection_round, executed + 1);
    assert!(
        next.delivered_round.is_some(),
        "the successor spreads on sn(c) + 1"
    );
}

#[test]
fn a_saturated_pool_completes_every_command_and_reuses_its_clients() {
    let report = run_smr(&scenario(COMPACT, 48, &[(1, 2)], 200));
    let clients: std::collections::BTreeSet<u32> =
        report.commands.iter().map(|c| c.client).collect();
    assert!(
        clients.len() <= 48,
        "arrivals never leave the registered pool"
    );
    assert!(
        report.commands.len() > 2 * clients.len(),
        "the run must reuse clients, not just fill the pool"
    );
    let peak = report
        .pool_peak_in_flight
        .expect("pool runs record their peak");
    assert!(
        (1..=48).contains(&peak),
        "peak in flight {peak} out of range"
    );
    let complete = report
        .commands
        .iter()
        .filter(|c| c.status == CommandStatus::Complete)
        .count();
    assert!(
        complete > report.commands.len() / 2,
        "a healthy population still drains a pooled stream"
    );
}

#[test]
fn a_unique_run_records_no_pool_peak() {
    let mut s = scenario(COMPACT, 48, &[(1, 1)], 40);
    s.client_model = ClientModel::Unique;
    assert_eq!(run_smr(&s).pool_peak_in_flight, None);
}

#[test]
fn an_undersized_pool_aborts_loudly() {
    let message = panic_message(|| {
        run_smr(&scenario(COMPACT, 1, &[(1, 2)], 40));
    });
    assert!(message.contains("client pool exhausted"), "got: {message}");
    assert!(
        message.contains("undersized"),
        "the message must name the cause — a registered K too small: {message}"
    );
}

#[test]
fn a_grid_point_that_exhausts_its_pool_is_an_abort_row() {
    // A direct run still panics (above); a grid records the point and goes on.
    let mut base: SmrScenarioSpec = serde_json::from_str(
        r#"{"n":16,"seed":0,"k":6,"ell":3,"sigma":5.0,
            "proto":{"kind":"compact","t_commit_rounds":8},
            "injections":[],"max_rounds":40,
            "client_model":{"kind":"pool","clients":1}}"#,
    )
    .expect("spec parses");
    base.traffic = Some(TrafficSpec::Pmf {
        arrivals_pmf: point_mass_pmf(2),
    });
    let spec = SmrGridSpec {
        exp: "pool-exhaustion".into(),
        base,
        pairs: None,
        betas: None,
        sigmas: None,
        rates: None,
        t_commits: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 16,
            seed_base: 1,
            seed_count: 1,
        }],
    };
    let csv = run_smr_grid_streaming(&spec, None, None).expect("the grid goes on");
    let row = csv.lines().nth(1).expect("one data row");
    assert!(row.contains(",abort,1,"), "aborted in round 1: {row}");
    assert!(row.contains(",pool,"), "kind pool: {row}");
}

#[test]
fn pool_runs_replay_identically() {
    let s = scenario(COMPACT, 48, &[(1, 2)], 120);
    assert_eq!(run_smr(&s), run_smr(&s));
}

const RECOVERY_ACK: Proto = Proto::Recovery {
    t_window_rounds: T_WINDOW,
    resend_until_acked: true,
    prefix_mismatch: sim::smr::PrefixMismatch::Abort,
};

// p. 25: the client sends "until commitment is acknowledged by the contacted server", so its
// slot frees on that ack, as in compact, not on the observer's all-executed latch.
#[test]
fn ack_mode_recovery_frees_a_pool_client_at_its_committed_ack_and_not_before() {
    let first = &single_arrival(RECOVERY_ACK).commands[0];
    let executed = first.executed_round.expect("executes inside the horizon");
    let acked = first.committed_ack_round.expect("acked inside the horizon");
    assert!(acked > executed, "precondition: the ack trails the latch");

    let at_latch = panic_message(|| {
        run_smr(&scenario(
            RECOVERY_ACK,
            1,
            &[(1, 1), (2, 0), (executed + 1, 1), (executed + 2, 0)],
            120,
        ));
    });
    assert!(
        at_latch.contains("client pool exhausted"),
        "the slot is busy until the ack, got: {at_latch}"
    );

    let report = run_smr(&scenario(
        RECOVERY_ACK,
        1,
        &[(1, 1), (2, 0), (acked + 1, 1), (acked + 2, 0)],
        120,
    ));
    let next = successor(&report, AUTO_CLIENT_BASE);
    assert_eq!(
        next.injection_round,
        acked + 1,
        "drawable the round after the ack"
    );
    assert!(
        next.delivered_round.is_some(),
        "the successor spreads on sn(c) + 1"
    );
    assert_eq!(report.pool_peak_in_flight, Some(1));
}

#[test]
fn an_ack_mode_recovery_pool_keeps_one_command_in_flight_per_client() {
    let report = run_smr(&scenario(RECOVERY_ACK, 64, &[(1, 1)], 200));
    let mut by_client: std::collections::BTreeMap<u32, Vec<&CommandReport>> =
        std::collections::BTreeMap::new();
    for c in &report.commands {
        by_client.entry(c.client).or_default().push(c);
    }
    assert!(
        report.commands.len() > by_client.len(),
        "the run must reuse clients"
    );
    for (client, mut cmds) in by_client {
        cmds.sort_by_key(|c| c.injection_round);
        for pair in cmds.windows(2) {
            let acked = pair[0]
                .committed_ack_round
                .expect("a successor exists only after an ack");
            assert!(
                pair[1].injection_round > acked,
                "client {client}: injected at {} before its predecessor's ack at {acked}",
                pair[1].injection_round
            );
        }
    }
}

#[test]
fn validation_rejects_pool_where_no_latch_frees_a_client() {
    let extended = scenario(Proto::Extended, 8, &[(1, 1)], 20);
    let err = extended.validate().expect_err("extended has no ack latch");
    assert!(err.contains("client_model pool"), "got: {err}");

    let empty = scenario(COMPACT, 0, &[(1, 1)], 20);
    assert!(
        empty.validate().is_err(),
        "a pool of zero clients can never serve an arrival"
    );
    assert!(scenario(COMPACT, 1, &[(1, 1)], 20).validate().is_ok());
    assert!(scenario(RECOVERY, 1, &[(1, 1)], 20).validate().is_ok());
}

#[test]
fn the_client_model_round_trips_through_the_wire_spec() {
    let json = r#"{"n":16,"seed":0,"k":6,"ell":3,"sigma":1.0,
        "proto":{"kind":"compact","t_commit_rounds":8},
        "injections":[],"max_rounds":20,
        "client_model":{"kind":"pool","clients":64}}"#;
    let spec: SmrScenarioSpec = serde_json::from_str(json).expect("spec parses");
    assert_eq!(spec.client_model, ClientModel::Pool { clients: 64 });
    let scenario = SmrScenario::try_from(spec.clone()).expect("valid scenario");
    assert_eq!(scenario.client_model, spec.client_model);
    let round_tripped: SmrScenarioSpec =
        serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
    assert_eq!(round_tripped.client_model, spec.client_model);

    // Absent field parses as unique and stays absent on the way out, so
    // committed specs and ledger rows are byte-identical.
    let without = r#"{"n":16,"seed":0,"k":6,"ell":3,"sigma":1.0,
        "proto":{"kind":"compact","t_commit_rounds":8},
        "injections":[],"max_rounds":20}"#;
    let unique: SmrScenarioSpec = serde_json::from_str(without).expect("old spec bytes parse");
    assert_eq!(unique.client_model, ClientModel::Unique);
    let emitted = serde_json::to_string(&unique).unwrap();
    assert!(!emitted.contains("client_model"), "got: {emitted}");
    let scenario_json = serde_json::to_string(&SmrScenario::try_from(unique).unwrap()).unwrap();
    assert!(
        !scenario_json.contains("client_model"),
        "got: {scenario_json}"
    );

    assert!(
        serde_json::from_str::<SmrScenarioSpec>(
            &json.replace(r#""clients":64"#, r#""clients":64,"size":64"#)
        )
        .is_err(),
        "unknown keys inside the model are rejected"
    );
}

#[test]
fn the_ledger_row_carries_the_pool_peak() {
    let dir = std::env::temp_dir().join(format!("smr-pool-ledger-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ledger = dir.join("ledger.jsonl");
    let s = scenario(COMPACT, 48, &[(1, 2)], 60);
    let report = run_smr(&s);
    sim::runlog::log_smr_run_to(&ledger, &s, &report, None).unwrap();

    let line = std::fs::read_to_string(&ledger).unwrap();
    let record: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
    assert_eq!(
        record["scenario"]["client_model"],
        serde_json::json!({"kind": "pool", "clients": 48}),
        "the run's client model must be regenerable from its ledger row"
    );
    assert_eq!(
        record["outcome"]["pool_peak_in_flight"],
        serde_json::json!(report.pool_peak_in_flight.unwrap()),
        "a near-miss on K must be visible without re-running"
    );
    std::fs::remove_dir_all(&dir).ok();
}
