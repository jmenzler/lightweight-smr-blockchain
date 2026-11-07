//! Byte-identity tripwire for every pre-existing `BlockSchedule` variant: the
//! per-round masks on all three draw paths (precomputed matrix, Alg-1 driver,
//! SMR driver) and both wire formats (spec JSON in, ledger JSON out). Committed
//! thesis runs replay from commit + seed + config, so these values were frozen
//! before the blocking strategies were refactored behind a trait.
//! Regenerate only as a deliberate, commit-noted act.

use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use sim::smr::{ClientModel, Proto, SmrScenario, TrafficPhase, run_smr};
use sim::spec::ScenarioSpec;
use sim::{
    AdaptivePolicy, BlockSchedule, BlockTarget, BlockWindow, Config, Init, Scenario, mask_schedule,
    run_traced,
};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn fold(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

const NS: [usize; 3] = [7, 20, 64];
const SEEDS: [u64; 3] = [1, 42, 9001];
// SMR seed 9001 under permanent_030 trips a pre-existing compact-engine debug assertion (canonical position vs front scan).
const SMR_SEEDS: [u64; 3] = [1, 42, 9002];

fn history_only_schedules() -> Vec<(&'static str, BlockSchedule)> {
    vec![
        ("fresh_000", BlockSchedule::FreshPerRound { fraction: 0.0 }),
        ("fresh_010", BlockSchedule::FreshPerRound { fraction: 0.1 }),
        ("fresh_025", BlockSchedule::FreshPerRound { fraction: 0.25 }),
        ("permanent_030", BlockSchedule::Permanent { fraction: 0.3 }),
        (
            "windows",
            BlockSchedule::Windows(vec![
                BlockWindow {
                    start_round: 2,
                    rounds: 3,
                    target: BlockTarget::Nodes(vec![0, 3, 5]),
                },
                BlockWindow {
                    start_round: 4,
                    rounds: 10,
                    target: BlockTarget::SampleFraction(0.2),
                },
                BlockWindow {
                    start_round: 1,
                    rounds: 30,
                    target: BlockTarget::SampleFraction(0.1),
                },
            ]),
        ),
        (
            "per_round",
            BlockSchedule::PerRoundFractions(vec![0.5, 0.0, 0.25, 0.1]),
        ),
        (
            "sticky",
            BlockSchedule::PerRoundSticky {
                background: 0.0,
                targets: vec![0.25, 0.25, 0.5, 0.1, 0.0, 0.3],
            },
        ),
        (
            "sticky_background",
            BlockSchedule::PerRoundSticky {
                background: 0.1,
                targets: vec![0.25, 0.25, 0.5, 0.1, 0.0, 0.3],
            },
        ),
    ]
}

fn node_state_schedules() -> Vec<(&'static str, BlockSchedule)> {
    let policy = AdaptivePolicy::BlockHolders;
    vec![
        (
            "adaptive_0late",
            BlockSchedule::Adaptive0Late {
                fraction: 0.2,
                policy,
            },
        ),
        (
            "alpha_1",
            BlockSchedule::AdaptiveAlphaLate {
                fraction: 0.2,
                alpha: 1,
                policy,
            },
        ),
        (
            "alpha_2",
            BlockSchedule::AdaptiveAlphaLate {
                fraction: 0.3,
                alpha: 2,
                policy,
            },
        ),
    ]
}

fn mask_bytes(mask: &[bool]) -> Vec<u8> {
    mask.iter().map(|&b| u8::from(b)).collect()
}

fn check(label: &str, actual: &[(&str, u64)], expected: &[(&str, u64)]) {
    let rendered: Vec<String> = actual
        .iter()
        .map(|(name, h)| format!("(\"{name}\", {h}),"))
        .collect();
    assert_eq!(
        actual,
        expected,
        "{label} digests changed; actual table:\n{}",
        rendered.join("\n")
    );
}

#[test]
fn mask_matrix_is_frozen_for_every_history_only_variant() {
    let actual: Vec<(&str, u64)> = history_only_schedules()
        .into_iter()
        .map(|(name, schedule)| {
            let mut h = FNV_OFFSET;
            for n in NS {
                for seed in SEEDS {
                    let masks =
                        mask_schedule(&schedule, n, 25, &mut ChaCha12Rng::seed_from_u64(seed));
                    for mask in &masks {
                        h = fold(h, &mask_bytes(mask));
                    }
                }
            }
            (name, h)
        })
        .collect();
    check(
        "mask_schedule",
        &actual,
        &[
            ("fresh_000", 15807897962019708735),
            ("fresh_010", 10750740042654378783),
            ("fresh_025", 5130008580772793723),
            ("permanent_030", 8839772678284500652),
            ("windows", 9174621524532520080),
            ("per_round", 16895197471684751398),
            ("sticky", 7179176624280217021),
            ("sticky_background", 1577990725981123376),
        ],
    );
}

fn alg1_scenario(n: usize, seed: u64, schedule: BlockSchedule) -> Scenario {
    Scenario {
        n,
        seed,
        cfg: Config::default(),
        init: Init::WithUndecided {
            useful_fraction: 0.7,
            inner: Box::new(Init::Split { fraction: 0.5 }),
        },
        max_rounds: 30,
        schedule,
        partition: None,
    }
}

#[test]
fn alg1_driver_masks_are_frozen_for_every_variant() {
    let actual: Vec<(&str, u64)> = history_only_schedules()
        .into_iter()
        .chain(node_state_schedules())
        .map(|(name, schedule)| {
            let mut h = FNV_OFFSET;
            for n in [20, 64] {
                for seed in SEEDS {
                    let (outcome, trace) = run_traced(&alg1_scenario(n, seed, schedule.clone()));
                    h = fold(h, format!("{outcome:?}").as_bytes());
                    for round in &trace.rounds {
                        h = fold(h, format!("{:?}", round.blocked).as_bytes());
                        h = fold(h, format!("{:?}", round.states).as_bytes());
                    }
                }
            }
            (name, h)
        })
        .collect();
    check(
        "alg1 driver",
        &actual,
        &[
            ("fresh_000", 630478539449994515),
            ("fresh_010", 2653423073684554351),
            ("fresh_025", 4331858005778597968),
            ("permanent_030", 13807700188244717065),
            ("windows", 2303131812064334697),
            ("per_round", 3147054072637967883),
            ("sticky", 15108880685723009657),
            ("sticky_background", 4854065300764084840),
            ("adaptive_0late", 9107398836095547247),
            ("alpha_1", 11636221278786150064),
            ("alpha_2", 2528037785152163895),
        ],
    );
}

fn smr_scenario(seed: u64, schedule: BlockSchedule) -> SmrScenario {
    SmrScenario {
        n: 20,
        seed,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Compact { t_commit_rounds: 8 },
        injections: vec![],
        max_rounds: 40,
        schedule,
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.5, 0.5],
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

#[test]
fn smr_driver_reports_are_frozen_for_every_history_only_variant() {
    let actual: Vec<(&str, u64)> = history_only_schedules()
        .into_iter()
        .map(|(name, schedule)| {
            let mut h = FNV_OFFSET;
            for seed in SMR_SEEDS {
                let scenario = smr_scenario(seed, schedule.clone());
                scenario.validate().expect("valid smr scenario");
                let report = run_smr(&scenario);
                h = fold(
                    h,
                    serde_json::to_string(&report)
                        .expect("report serializes")
                        .as_bytes(),
                );
            }
            (name, h)
        })
        .collect();
    check(
        "smr driver",
        &actual,
        &[
            ("fresh_000", 16038731067604528578),
            ("fresh_010", 3720232516699797464),
            ("fresh_025", 2648352413165425169),
            ("permanent_030", 3930823159045930997),
            ("windows", 10030392588540544323),
            ("per_round", 5688651735787265634),
            ("sticky", 3563714626661213900),
            ("sticky_background", 2529322324254825798),
        ],
    );
}

const SPEC_SCHEDULES: [&str; 8] = [
    r#"{"kind":"fresh_per_round","fraction":0.1}"#,
    r#"{"kind":"permanent","fraction":0.3}"#,
    r#"{"kind":"windows","windows":[{"start_round":2,"rounds":3,"target":{"kind":"nodes","ids":[0,3]}},{"start_round":1,"rounds":9,"target":{"kind":"sample_fraction","fraction":0.2}}]}"#,
    r#"{"kind":"per_round_fractions","fractions":[0.5,0.0,0.25]}"#,
    r#"{"kind":"per_round_sticky","fractions":[0.25,0.5],"background":0.1}"#,
    r#"{"kind":"per_round_sticky","fractions":[0.25,0.5]}"#,
    r#"{"kind":"adaptive_0late","fraction":0.2,"policy":"block_holders"}"#,
    r#"{"kind":"adaptive_alpha_late","fraction":0.2,"alpha":1,"policy":"block_holders"}"#,
];

const SPEC_SCHEDULES_REWRITTEN: [&str; 8] = [
    r#"{"kind":"fresh_per_round","fraction":0.1}"#,
    r#"{"kind":"permanent","fraction":0.3}"#,
    r#"{"kind":"windows","windows":[{"start_round":2,"rounds":3,"target":{"kind":"nodes","ids":[0,3]}},{"start_round":1,"rounds":9,"target":{"kind":"sample_fraction","fraction":0.2}}]}"#,
    r#"{"kind":"per_round_fractions","fractions":[0.5,0.0,0.25]}"#,
    r#"{"kind":"per_round_sticky","fractions":[0.25,0.5],"background":0.1}"#,
    r#"{"kind":"per_round_sticky","fractions":[0.25,0.5],"background":0.0}"#,
    r#"{"kind":"adaptive_0late","fraction":0.2,"policy":"block_holders"}"#,
    r#"{"kind":"adaptive_alpha_late","fraction":0.2,"alpha":1,"policy":"block_holders"}"#,
];

const LEDGER_SCHEDULES: [&str; 8] = [
    r#"{"FreshPerRound":{"fraction":0.1}}"#,
    r#"{"Permanent":{"fraction":0.3}}"#,
    r#"{"Windows":[{"start_round":2,"rounds":3,"target":{"Nodes":[0,3]}},{"start_round":1,"rounds":9,"target":{"SampleFraction":0.2}}]}"#,
    r#"{"PerRoundFractions":[0.5,0.0,0.25]}"#,
    r#"{"PerRoundSticky":{"background":0.1,"targets":[0.25,0.5]}}"#,
    r#"{"PerRoundSticky":{"background":0.0,"targets":[0.25,0.5]}}"#,
    r#"{"Adaptive0Late":{"fraction":0.2,"policy":"block_holders"}}"#,
    r#"{"AdaptiveAlphaLate":{"fraction":0.2,"alpha":1,"policy":"block_holders"}}"#,
];

#[test]
fn schedule_wire_formats_are_frozen() {
    for ((input, rewritten), ledger) in SPEC_SCHEDULES
        .iter()
        .zip(SPEC_SCHEDULES_REWRITTEN)
        .zip(LEDGER_SCHEDULES)
    {
        let json = format!(
            r#"{{"n":20,"seed":7,"k":6,"ell":3,"init":{{"kind":"split","fraction":0.5}},"max_rounds":50,"schedule":{input}}}"#
        );
        let spec: ScenarioSpec = serde_json::from_str(&json).expect("spec parses");
        assert_eq!(
            serde_json::to_string(&spec).expect("spec serializes"),
            json.replace(input, rewritten),
            "spec round-trip for {input}"
        );
        let scenario = Scenario::try_from(spec).expect("spec converts");
        assert_eq!(
            serde_json::to_string(&scenario.schedule).expect("schedule serializes"),
            ledger,
            "ledger form of {input}"
        );
    }
}

#[test]
fn scenario_ledger_row_is_frozen() {
    let scenario = alg1_scenario(
        20,
        7,
        BlockSchedule::AdaptiveAlphaLate {
            fraction: 0.2,
            alpha: 1,
            policy: AdaptivePolicy::BlockHolders,
        },
    );
    assert_eq!(
        serde_json::to_string(&scenario).expect("scenario serializes"),
        r#"{"n":20,"seed":7,"cfg":{"k":6,"ell":3},"init":{"WithUndecided":{"useful_fraction":0.7,"inner":{"Split":{"fraction":0.5}}}},"max_rounds":30,"schedule":{"AdaptiveAlphaLate":{"fraction":0.2,"alpha":1,"policy":"block_holders"}}}"#
    );
}
