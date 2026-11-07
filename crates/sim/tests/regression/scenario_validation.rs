//! A directly built `Scenario` is held to the same rules as one parsed from a spec: a typo in a
//! hand-written runner must stop the run, not silently run another adversary.

use sim::{AdaptivePolicy, BlockSchedule, BlockTarget, BlockWindow, Config, Init, Scenario, run};

fn scenario(init: Init, schedule: BlockSchedule) -> Scenario {
    Scenario {
        n: 10,
        seed: 1,
        cfg: Config::default(),
        init,
        max_rounds: 20,
        schedule,
        partition: None,
    }
}

fn window(start_round: usize, rounds: usize, target: BlockTarget) -> BlockSchedule {
    BlockSchedule::Windows(vec![BlockWindow {
        start_round,
        rounds,
        target,
    }])
}

fn calm() -> BlockSchedule {
    BlockSchedule::FreshPerRound { fraction: 0.0 }
}

#[test]
fn rejects_what_would_run_another_adversary_or_panic_deep_in_the_loop() {
    let split = || Init::Split { fraction: 0.5 };
    let nodes = |ids: Vec<u32>| BlockTarget::Nodes(ids);
    let cases: Vec<(&str, Scenario)> = vec![
        (
            "window start 0",
            scenario(split(), window(0, 0, nodes(vec![3]))),
        ),
        (
            "window rounds 0",
            scenario(split(), window(5, 0, nodes(vec![3]))),
        ),
        (
            "window end overflows",
            scenario(split(), window(2, usize::MAX, nodes(vec![3]))),
        ),
        (
            "window id out of range",
            scenario(split(), window(1, 2, nodes(vec![10]))),
        ),
        (
            "window sample fraction above 1",
            scenario(split(), window(1, 2, BlockTarget::SampleFraction(1.5))),
        ),
        (
            "alpha 0",
            scenario(
                split(),
                BlockSchedule::AdaptiveAlphaLate {
                    fraction: 0.1,
                    alpha: 0,
                    policy: AdaptivePolicy::BlockHolders,
                },
            ),
        ),
        (
            "fresh fraction negative",
            scenario(split(), BlockSchedule::FreshPerRound { fraction: -0.1 }),
        ),
        (
            "uniform k 0",
            scenario(Init::UniformRandom { k: 0 }, calm()),
        ),
        (
            "even split 0 values",
            scenario(Init::EvenSplit { values: 0 }, calm()),
        ),
        (
            "weighted no buckets",
            scenario(
                Init::Weighted {
                    weights: vec![],
                    range: 4,
                },
                calm(),
            ),
        ),
        (
            "split fraction above 1",
            scenario(Init::Split { fraction: 1.2 }, calm()),
        ),
        (
            "n 0",
            Scenario {
                n: 0,
                ..scenario(split(), calm())
            },
        ),
    ];
    for (name, sc) in cases {
        assert!(sc.validate().is_err(), "{name} must be rejected");
        assert!(
            std::panic::catch_unwind(|| run(&sc)).is_err(),
            "{name}: run must stop instead of running"
        );
    }
}

#[test]
fn accepts_a_well_formed_scenario() {
    let sc = scenario(
        Init::Split { fraction: 0.5 },
        window(3, 2, BlockTarget::Nodes(vec![0, 9])),
    );
    assert!(sc.validate().is_ok());
}
