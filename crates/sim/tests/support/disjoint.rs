//! `BlockSchedule::Disjoint` at the public surfaces: spec JSON, ledger form, SMR validation, and
//! the Alg-1 driver. Exploratory strategy (worst-case 1-late availability adversary,
//! thesis/supervisor/optimal-adversary), not registered thesis evidence.

use sim::smr::{ClientModel, Proto, SmrScenario};
use sim::spec::ScenarioSpec;
use sim::{BlockSchedule, Config, Init, Scenario, run_traced};

fn spec_json(schedule: &str) -> String {
    format!(
        r#"{{"n":20,"seed":7,"k":6,"ell":3,"init":{{"kind":"split","fraction":0.5}},"max_rounds":50,"schedule":{schedule}}}"#
    )
}

#[test]
fn disjoint_spec_round_trips_and_has_a_stable_ledger_form() {
    let json = spec_json(r#"{"kind":"disjoint","fraction":0.1}"#);
    let spec: ScenarioSpec = serde_json::from_str(&json).expect("spec parses");
    assert_eq!(serde_json::to_string(&spec).unwrap(), json);
    let scenario = Scenario::try_from(spec).expect("spec converts");
    assert_eq!(scenario.schedule, BlockSchedule::Disjoint { fraction: 0.1 });
    assert_eq!(
        serde_json::to_string(&scenario.schedule).unwrap(),
        r#"{"Disjoint":{"fraction":0.1}}"#
    );
}

#[test]
fn disjoint_spec_above_one_half_is_rejected() {
    let spec: ScenarioSpec =
        serde_json::from_str(&spec_json(r#"{"kind":"disjoint","fraction":0.6}"#)).unwrap();
    let err = Scenario::try_from(spec).unwrap_err();
    assert!(err.contains("fraction <= 1/2"), "{err}");
    let spec: ScenarioSpec =
        serde_json::from_str(&spec_json(r#"{"kind":"disjoint","fraction":0.5}"#)).unwrap();
    assert!(Scenario::try_from(spec).is_ok());
}

fn smr(schedule: BlockSchedule) -> SmrScenario {
    SmrScenario {
        n: 20,
        seed: 3,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Compact { t_commit_rounds: 8 },
        injections: vec![],
        max_rounds: 30,
        schedule,
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

#[test]
fn smr_accepts_disjoint_because_it_reads_only_its_own_blocks() {
    smr(BlockSchedule::Disjoint { fraction: 0.2 })
        .validate()
        .expect("history-only strategy passes SMR validation");
    let err = smr(BlockSchedule::Disjoint { fraction: 0.6 })
        .validate()
        .unwrap_err();
    assert!(err.contains("fraction <= 1/2"), "{err}");
    let err = smr(BlockSchedule::AdaptiveAlphaLate {
        fraction: 0.2,
        alpha: 1,
        policy: sim::AdaptivePolicy::BlockHolders,
    })
    .validate()
    .unwrap_err();
    assert!(err.contains("Alg-1 only"), "{err}");
}

#[test]
fn alg1_driver_applies_disjoint_masks_and_its_useful_count_is_n_minus_2b_bounded() {
    let n = 40;
    let b = 6;
    let scenario = Scenario {
        n,
        seed: 11,
        cfg: Config::default(),
        init: Init::Split { fraction: 0.5 },
        max_rounds: 25,
        schedule: BlockSchedule::Disjoint { fraction: 0.15 },
        partition: None,
    };
    let (_, trace) = run_traced(&scenario);
    let report = sim::run_report(&scenario);
    assert_eq!(
        report.metrics[0].useful as usize,
        n - b,
        "round 1: everyone holds"
    );
    for (t, round) in trace.rounds.iter().enumerate() {
        assert_eq!(round.blocked.len(), b);
        assert!(report.metrics[t].useful as usize <= n - b);
        if t > 0 {
            assert!(
                report.metrics[t].useful as usize <= n - 2 * b,
                "round {}: B_t-1 and B_t are disjoint, so at most n - 2b can be useful",
                t + 1
            );
            assert!(
                round
                    .blocked
                    .iter()
                    .all(|id| !trace.rounds[t - 1].blocked.contains(id))
            );
        }
    }
}
