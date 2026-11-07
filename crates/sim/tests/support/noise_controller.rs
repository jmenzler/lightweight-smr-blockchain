//! `BlockSchedule::NoiseController` at the public surfaces: spec JSON, ledger form, SMR
//! refusal, refusal of scenarios outside the DP model, and a trace replay of every action
//! against the table file.

use sim::smr::{ClientModel, Proto, SmrScenario};
use sim::spec::ScenarioSpec;
use sim::{BlockSchedule, Config, Init, Scenario, run_report, run_traced};
use std::collections::{BTreeSet, HashMap};

fn spec_json(n: usize, schedule: &str) -> String {
    format!(
        r#"{{"n":{n},"seed":7,"k":6,"ell":3,"init":{{"kind":"uniform_random","k":2}},"max_rounds":50,"schedule":{schedule}}}"#
    )
}

#[test]
fn noise_controller_spec_round_trips_and_has_a_stable_ledger_form() {
    let json = spec_json(50, r#"{"kind":"noise_controller","fraction":0.1}"#);
    let spec: ScenarioSpec = serde_json::from_str(&json).expect("spec parses");
    assert_eq!(serde_json::to_string(&spec).unwrap(), json);
    let scenario = Scenario::try_from(spec).expect("spec converts");
    assert_eq!(
        serde_json::to_string(&scenario.schedule).unwrap(),
        r#"{"NoiseController":{"fraction":0.1}}"#
    );
}

#[test]
fn noise_controller_spec_without_a_table_is_rejected() {
    let spec: ScenarioSpec = serde_json::from_str(&spec_json(
        40,
        r#"{"kind":"noise_controller","fraction":0.1}"#,
    ))
    .unwrap();
    let err = Scenario::try_from(spec).unwrap_err();
    assert!(
        err.contains("no noise_controller table for n=40, b=4"),
        "{err}"
    );
}

fn noise_controller_scenario_json(k: usize, ell: usize, init: &str, partition: &str) -> String {
    format!(
        r#"{{"n":50,"seed":7,"k":{k},"ell":{ell},"init":{init},"max_rounds":50,"schedule":{{"kind":"noise_controller","fraction":0.1}}{partition}}}"#
    )
}

const UNIFORM_TWO: &str = r#"{"kind":"uniform_random","k":2}"#;

fn conversion_error(json: &str) -> String {
    let spec: ScenarioSpec = serde_json::from_str(json).expect("spec parses");
    Scenario::try_from(spec).expect_err("the DP does not model this scenario")
}

#[test]
fn noise_controller_spec_refuses_rules_other_than_6_3() {
    for (k, ell) in [(8, 5), (6, 5), (7, 3)] {
        let err = conversion_error(&noise_controller_scenario_json(k, ell, UNIFORM_TWO, ""));
        assert!(err.contains("(6,3)"), "k={k} ell={ell}: {err}");
    }
}

#[test]
fn noise_controller_spec_refuses_a_partition() {
    let partition = format!(r#","partition":[{}]"#, vec!["0"; 50].join(","));
    let err = conversion_error(&noise_controller_scenario_json(
        6,
        3,
        UNIFORM_TWO,
        &partition,
    ));
    assert!(err.contains("partition"), "{err}");
}

#[test]
fn noise_controller_spec_refuses_inits_other_than_iid_uniform_two_values() {
    for init in [
        r#"{"kind":"split","fraction":0.5}"#,
        r#"{"kind":"even_split","values":2}"#,
        r#"{"kind":"uniform_random","k":3}"#,
        r#"{"kind":"weighted","weights":[1.0,1.0]}"#,
        r#"{"kind":"distinct"}"#,
        r#"{"kind":"with_undecided","useful_fraction":0.9,"inner":{"kind":"uniform_random","k":2}}"#,
    ] {
        let err = conversion_error(&noise_controller_scenario_json(6, 3, init, ""));
        assert!(err.contains("uniform_random k=2"), "{init}: {err}");
    }
}

#[test]
fn sweep_points_outside_the_dp_model_are_refused() {
    let spec: sim::sweep::GridSpec = serde_json::from_str(&format!(
        r#"{{"exp":"t","base":{},"pairs":[[6,3],[9,5]],"ladder":[{{"n":50,"seed_base":0,"seed_count":1}}]}}"#,
        noise_controller_scenario_json(6, 3, UNIFORM_TWO, "")
    ))
    .expect("grid spec parses");
    let err = sim::sweep::expand(&spec).expect_err("(9,5) point is outside the DP model");
    assert!(err.contains("(6,3)"), "{err}");
}

#[test]
#[should_panic(expected = "noise_controller tables are solved for the (6,3) rule")]
fn alg1_driver_refuses_a_directly_built_scenario_outside_the_dp_model() {
    run_report(&Scenario {
        n: 50,
        seed: 1,
        cfg: Config::new(9, 5).unwrap(),
        init: Init::UniformRandom { k: 2 },
        max_rounds: 5,
        schedule: BlockSchedule::NoiseController { fraction: 0.1 },
        partition: None,
    });
}

fn smr(n: usize) -> SmrScenario {
    SmrScenario {
        n,
        seed: 3,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Compact { t_commit_rounds: 8 },
        injections: vec![],
        max_rounds: 30,
        schedule: BlockSchedule::NoiseController { fraction: 0.1 },
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
fn smr_rejects_the_noise_controller_because_it_reads_node_state() {
    let err = smr(50).validate().unwrap_err();
    assert!(err.contains("Alg-1 only"), "{err}");
    let err = smr(40).validate().unwrap_err();
    assert!(err.contains("no noise_controller table"), "{err}");
}

#[test]
#[should_panic(expected = "iid two-value start")]
fn alg1_driver_refuses_a_three_value_start_before_round_one() {
    run_report(&Scenario {
        n: 30,
        seed: 1,
        cfg: Config::default(),
        init: Init::EvenSplit { values: 3 },
        max_rounds: 5,
        schedule: BlockSchedule::NoiseController { fraction: 0.1 },
        partition: None,
    });
}

/// Independent reading of a table file: (s, u, a1) -> (m, o); absent states block nobody.
fn table_file(n: usize, b: usize) -> HashMap<(usize, usize, usize), (usize, usize)> {
    let path = format!(
        "{}/data/noise_controller/n{n}_b{b}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut map = HashMap::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        if line.starts_with('#') {
            continue;
        }
        let f: Vec<usize> = line
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        for a1 in f[2]..=f[3] {
            map.insert((f[0], f[1], a1), (f[4], f[5]));
        }
    }
    map
}

const TABLES: [(usize, usize); 6] = [(30, 3), (50, 5), (50, 7), (100, 10), (100, 14), (200, 20)];

#[test]
fn every_action_replays_the_table_on_the_one_round_old_useful_set() {
    for (n, b) in TABLES {
        let table = table_file(n, b);
        let has_reblocks = table.values().any(|&(_, o)| o > 0);
        let mut played = [0usize; 2];
        for seed in 0..200 {
            let (_, trace) = run_traced(&Scenario {
                n,
                seed,
                cfg: Config::default(),
                init: Init::UniformRandom { k: 2 },
                max_rounds: 30,
                schedule: BlockSchedule::NoiseController {
                    fraction: b as f64 / n as f64,
                },
                partition: None,
            });
            let mut previous: BTreeSet<u32> = BTreeSet::new();
            let mut known = (n, n / 2);
            let mut before = trace.initial.clone();
            for round in &trace.rounds {
                let blocked: BTreeSet<u32> = round.blocked.iter().copied().collect();
                let want = table
                    .get(&(previous.len(), known.0, known.1))
                    .copied()
                    .unwrap_or((0, 0));
                let got = (
                    blocked.difference(&previous).count(),
                    blocked.intersection(&previous).count(),
                );
                assert_eq!(
                    got,
                    want,
                    "n={n} b={b} seed {seed}: state {:?}",
                    (previous.len(), known)
                );
                played[0] += usize::from(got.0 > 0);
                played[1] += usize::from(got.1 > 0);
                let useful: Vec<u64> = (0..n as u32)
                    .filter(|i| !blocked.contains(i))
                    .filter_map(|i| before[i as usize])
                    .collect();
                let larger = useful.iter().max().copied();
                known = (
                    useful.len(),
                    useful.iter().filter(|&&v| Some(v) == larger).count(),
                );
                previous = blocked;
                before.clone_from(&round.states);
            }
        }
        assert!(played[0] > 0, "n={n} b={b}: no fresh block exercised");
        assert!(
            !has_reblocks || played[1] > 0,
            "n={n} b={b}: table re-blocks but the replay never did: {played:?}"
        );
    }
}

/// The replay's "larger value" convention (and the code's) is immaterial only if the DP's action
/// is symmetric in the two values: action(s, u, a1) = action(s, u, u − a1) for every state.
#[test]
fn every_table_is_symmetric_in_the_two_values() {
    for (n, b) in TABLES {
        let table = table_file(n, b);
        for (&(s, u, a1), action) in &table {
            assert_eq!(
                table.get(&(s, u, u - a1)),
                Some(action),
                "n={n} b={b}: state (s={s}, u={u}, a1={a1}) has no mirrored action"
            );
        }
    }
}
