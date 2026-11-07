use sim::spec::{InitSpec, ScenarioSpec, ScheduleSpec};
use sim::sweep::{GridSpec, LadderRung, expand, run_grid_streaming};
use sim::{AdaptivePolicy, BlockSchedule};

fn base(init: InitSpec, schedule: Option<ScheduleSpec>) -> ScenarioSpec {
    ScenarioSpec {
        n: 200,
        seed: 0,
        k: 6,
        ell: 3,
        init,
        max_rounds: 2000,
        schedule,
        partition: None,
    }
}

fn split_fresh_base() -> ScenarioSpec {
    base(
        InitSpec::Split { fraction: 0.5 },
        Some(ScheduleSpec::FreshPerRound { fraction: 0.0 }),
    )
}

fn smoke_spec() -> GridSpec {
    GridSpec {
        exp: "smoke".into(),
        base: split_fresh_base(),
        pairs: None,
        betas: Some(vec![0.100, 0.150]),
        useful_fractions: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 200,
            seed_base: 10000,
            seed_count: 3,
        }],
    }
}

#[test]
fn expand_orders_points_deterministically_and_validates() {
    let spec = GridSpec {
        exp: "t".into(),
        base: split_fresh_base(),
        pairs: Some(vec![(6, 3), (8, 5)]),
        betas: Some(vec![0.1, 0.2]),
        useful_fractions: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 50,
            seed_base: 100,
            seed_count: 2,
        }],
    };
    let points = expand(&spec).expect("valid spec");
    assert_eq!(points.len(), 2 * 2 * 2);
    // fixed nesting: pairs -> ladder -> betas -> seeds
    let keys: Vec<_> = points
        .iter()
        .map(|(key, _)| (key.k, key.ell, key.n, key.beta, key.seed))
        .collect();
    assert_eq!(keys[0], (6, 3, 50, Some(0.1), 100));
    assert_eq!(keys[1], (6, 3, 50, Some(0.1), 101));
    assert_eq!(keys[2], (6, 3, 50, Some(0.2), 100));
    assert_eq!(keys[4], (8, 5, 50, Some(0.1), 100));
    // scenarios carry the overrides
    assert_eq!(points[4].1.cfg.k, 8);
    assert_eq!(points[4].1.n, 50);

    let bad = GridSpec {
        pairs: Some(vec![(6, 4)]),
        ..spec
    };
    assert!(expand(&bad).is_err(), "even ell must be rejected");
}

#[test]
fn betas_axis_requires_fraction_schedule() {
    let spec = GridSpec {
        exp: "t".into(),
        base: base(InitSpec::Split { fraction: 0.5 }, None),
        pairs: None,
        betas: Some(vec![0.1]),
        useful_fractions: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 50,
            seed_base: 0,
            seed_count: 1,
        }],
    };
    assert!(
        expand(&spec).is_err(),
        "betas axis without a fraction schedule must be rejected"
    );
}

#[test]
fn useful_axis_requires_with_undecided() {
    let spec = GridSpec {
        exp: "t".into(),
        base: split_fresh_base(),
        pairs: None,
        betas: None,
        useful_fractions: Some(vec![0.6]),
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 50,
            seed_base: 0,
            seed_count: 1,
        }],
    };
    assert!(
        expand(&spec).is_err(),
        "useful_fractions axis requires a with_undecided init"
    );
}

#[test]
fn useful_axis_overrides_with_undecided_fraction() {
    let spec = GridSpec {
        exp: "t".into(),
        base: base(
            InitSpec::WithUndecided {
                useful_fraction: 1.0,
                inner: Box::new(InitSpec::Split { fraction: 0.5 }),
            },
            Some(ScheduleSpec::FreshPerRound { fraction: 0.1 }),
        ),
        pairs: None,
        betas: None,
        useful_fractions: Some(vec![0.56, 0.70]),
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 50,
            seed_base: 0,
            seed_count: 1,
        }],
    };
    let points = expand(&spec).expect("valid spec");
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].0.useful_fraction, Some(0.56));
    match &points[0].1.init {
        sim::Init::WithUndecided {
            useful_fraction, ..
        } => assert_eq!(*useful_fraction, 0.56),
        other => panic!("expected WithUndecided, got {other:?}"),
    }
}

#[test]
fn parallel_output_is_byte_identical_to_single_thread() {
    let dir = std::env::temp_dir().join(format!("sweep-grid-par-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let spec = smoke_spec();
    let single = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("pool")
        .install(|| run_grid_streaming(&spec, Some(&ledger), None).expect("grid runs"));
    let parallel = run_grid_streaming(&spec, Some(&ledger), None).expect("grid runs");
    assert_eq!(single, parallel, "CSV must not depend on thread scheduling");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn grid_run_appends_one_ledger_row_per_run_and_matches_smoke_anchor() {
    let dir = std::env::temp_dir().join(format!("sweep-grid-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let _ = std::fs::remove_file(&ledger);

    let csv = run_grid_streaming(&smoke_spec(), Some(&ledger), None).expect("grid runs");

    let rows: Vec<&str> = csv.lines().collect();
    assert_eq!(rows[0], sim::sweep::CSV_HEADER);
    assert_eq!(rows.len(), 1 + 6);
    // historical sweep.rs smoke anchor: beta=0.100 -> 3/3 agreement, 0.150 -> 0/3
    let agreements_low = rows[1..4]
        .iter()
        .filter(|r| r.contains(",agreement,"))
        .count();
    let agreements_high = rows[4..7]
        .iter()
        .filter(|r| r.contains(",agreement,"))
        .count();
    assert_eq!((agreements_low, agreements_high), (3, 0));

    let ledger_lines = std::fs::read_to_string(&ledger).expect("ledger written");
    assert_eq!(ledger_lines.lines().count(), 6);
    assert!(ledger_lines.contains("\"exp\":\"smoke\""));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn horizons_axis_overrides_max_rounds() {
    let spec = GridSpec {
        exp: "t".into(),
        base: split_fresh_base(),
        pairs: None,
        betas: None,
        useful_fractions: None,
        horizons: Some(vec![500, 2000]),
        splits: None,
        ladder: vec![LadderRung {
            n: 50,
            seed_base: 0,
            seed_count: 1,
        }],
    };
    let points = sim::sweep::expand(&spec).expect("valid spec");
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].0.horizon, Some(500));
    assert_eq!(points[0].1.max_rounds, 500);
    assert_eq!(points[1].1.max_rounds, 2000);
}

#[test]
fn spec_json_parses_single_and_array_of_same_kind() {
    use sim::sweep::{AnySpec, parse_specs};
    let single = serde_json::to_string(&smoke_spec()).unwrap();
    let parsed = parse_specs(&single).expect("single parses");
    assert_eq!(parsed.len(), 1);
    assert!(matches!(parsed[0], AnySpec::Grid(_)));

    let array = format!("[{single},{single}]");
    let parsed = parse_specs(&array).expect("array parses");
    assert_eq!(parsed.len(), 2);
}

#[test]
fn a_spec_parse_error_names_the_missing_field() {
    use sim::sweep::parse_specs;
    let smr = r#"{"exp":"e","base":{"n":16,"seed":1,"k":6,"ell":3,"sigma":1.0,
        "proto":{"kind":"recovery","t_window_rounds":8},"injections":[],"max_rounds":10},
        "ladder":[{"n":16,"seed_base":0,"seed_count":1}]}"#;
    let err = parse_specs(smr).expect_err("resend_until_acked is required");
    assert!(err.contains("resend_until_acked"), "{err}");
    let err = parse_specs(&format!("[{smr}]")).expect_err("also inside an array");
    assert!(err.contains("resend_until_acked"), "{err}");

    let mut grid = serde_json::to_value(smoke_spec()).unwrap();
    grid["base"].as_object_mut().unwrap().remove("max_rounds");
    let err = parse_specs(&grid.to_string()).expect_err("max_rounds is required");
    assert!(err.contains("max_rounds"), "{err}");

    let err =
        parse_specs(r#"{"exp":"e","base":{},"ladder":[]}"#).expect_err("neither kind of spec");
    assert!(err.contains("init") && err.contains("proto"), "{err}");
}

#[test]
fn streaming_partial_file_holds_rows_and_final_csv_is_byte_identical() {
    use sim::sweep::run_grid_streaming;
    let dir = std::env::temp_dir().join(format!("sweep-grid-stream-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let partial = dir.join("out.csv.partial");
    let _ = std::fs::remove_file(&partial);
    let spec = smoke_spec();

    let reference = run_grid_streaming(&spec, Some(&ledger), None).expect("grid runs");
    let streamed = run_grid_streaming(&spec, Some(&ledger), Some(&partial)).expect("grid runs");

    assert_eq!(streamed, reference, "streaming must not change the CSV");
    let on_disk = std::fs::read_to_string(&partial).expect("partial written");
    // partial carries rows only (no header) — crash-surviving row store
    let rows = reference.split_once('\n').map(|(_, r)| r).unwrap();
    assert_eq!(on_disk, rows);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn streaming_appends_to_existing_partial_without_header() {
    use sim::sweep::run_grid_streaming;
    let dir = std::env::temp_dir().join(format!("sweep-grid-append-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let partial = dir.join("out.csv.partial");
    std::fs::write(&partial, "preexisting\n").expect("seed file");
    let spec = smoke_spec();

    let csv = run_grid_streaming(&spec, Some(&ledger), Some(&partial)).expect("grid runs");
    let on_disk = std::fs::read_to_string(&partial).expect("partial");
    let rows = csv.split_once('\n').map(|(_, r)| r).unwrap();
    assert_eq!(on_disk, format!("preexisting\n{rows}"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn betas_axis_drives_adaptive_schedule() {
    let spec = GridSpec {
        exp: "t".into(),
        base: base(
            InitSpec::Split { fraction: 0.5 },
            Some(ScheduleSpec::Adaptive0Late {
                fraction: 0.0,
                policy: AdaptivePolicy::BlockHolders,
            }),
        ),
        pairs: None,
        betas: Some(vec![0.11]),
        useful_fractions: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 50,
            seed_base: 0,
            seed_count: 1,
        }],
    };
    let points = expand(&spec).expect("adaptive base must accept a betas axis");
    assert_eq!(
        points[0].1.schedule,
        BlockSchedule::Adaptive0Late {
            fraction: 0.11,
            policy: AdaptivePolicy::BlockHolders,
        },
        "the beta must land on the adaptive schedule, not swap it for fresh"
    );
}

#[test]
fn split_partition_uses_nearest_target_size() {
    use sim::sweep::split_partition;
    assert_eq!(split_partition(0.7, 10), vec![0, 0, 0, 0, 0, 0, 0, 1, 1, 1]);
    assert_eq!(split_partition(0.75, 4), vec![0, 0, 0, 1]);
    assert_eq!(split_partition(0.5, 3), vec![0, 0, 1]);
}

#[test]
fn splits_axis_lands_a_contiguous_two_block_partition() {
    let spec = GridSpec {
        exp: "t".into(),
        base: split_fresh_base(),
        pairs: None,
        betas: None,
        useful_fractions: None,
        horizons: None,
        splits: Some(vec![0.7, 0.75]),
        ladder: vec![LadderRung {
            n: 40,
            seed_base: 0,
            seed_count: 1,
        }],
    };
    let points = expand(&spec).expect("valid spec");
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].0.split, Some(0.7));
    let partition = points[0].1.partition.as_ref().expect("partition set");
    assert_eq!(partition.len(), 40);
    assert!(partition[..28].iter().all(|&c| c == 0));
    assert!(partition[28..].iter().all(|&c| c == 1));
    let wider = points[1].1.partition.as_ref().expect("partition set");
    assert_eq!(wider.iter().filter(|&&c| c == 0).count(), 30);
}

#[test]
fn splits_axis_rejects_a_base_that_already_names_a_partition() {
    let mut named = split_fresh_base();
    named.partition = Some(vec![0; 200]);
    let spec = GridSpec {
        exp: "t".into(),
        base: named,
        pairs: None,
        betas: None,
        useful_fractions: None,
        horizons: None,
        splits: Some(vec![0.7]),
        ladder: vec![LadderRung {
            n: 200,
            seed_base: 0,
            seed_count: 1,
        }],
    };
    assert!(
        expand(&spec).is_err(),
        "a splits axis over an explicit base partition is ambiguous"
    );
}

#[test]
fn splits_axis_nests_innermost_before_seeds() {
    let spec = GridSpec {
        exp: "t".into(),
        base: split_fresh_base(),
        pairs: None,
        betas: None,
        useful_fractions: None,
        horizons: Some(vec![10, 20]),
        splits: Some(vec![0.6, 0.7]),
        ladder: vec![LadderRung {
            n: 40,
            seed_base: 0,
            seed_count: 2,
        }],
    };
    let keys: Vec<_> = expand(&spec)
        .expect("valid spec")
        .iter()
        .map(|(k, _)| (k.horizon, k.split, k.seed))
        .collect();
    assert_eq!(keys[0], (Some(10), Some(0.6), 0));
    assert_eq!(keys[1], (Some(10), Some(0.6), 1));
    assert_eq!(keys[2], (Some(10), Some(0.7), 0));
    assert_eq!(keys[4], (Some(20), Some(0.6), 0));
}

/// The runner's stderr tally keys on every axis column, so two arms of one
/// grid can never merge into a single line — and it reads the outcome column
/// by name, so a new key column cannot silently make it count the wrong one.
#[test]
fn the_runner_summary_keys_on_every_axis_and_counts_survivors() {
    let csv = format!(
        "{}\n{}\n{}\n{}\n",
        sim::sweep::CSV_HEADER,
        "t,6,3,40,,,,0.700,1,died,13",
        "t,6,3,40,,,,0.750,1,agreement,9",
        "t,6,3,40,,,,0.750,2,agreement,10",
    );
    let lines = sim::sweep::grid_summary(&csv);
    assert_eq!(
        lines,
        vec![
            "(6,3) n=40 beta= useful_fraction= horizon= split=0.700: 0/1 survived".to_string(),
            "(6,3) n=40 beta= useful_fraction= horizon= split=0.750: 2/2 survived".to_string(),
        ]
    );
}

#[test]
fn the_split_column_sits_between_horizon_and_seed() {
    let cols: Vec<&str> = sim::sweep::CSV_HEADER.split(',').collect();
    let at = cols
        .iter()
        .position(|&c| c == "split")
        .expect("split column");
    assert_eq!((cols[at - 1], cols[at + 1]), ("horizon", "seed"));
}

#[test]
fn grid_points_finer_than_three_decimals_stay_separate_cells() {
    let spec = GridSpec {
        betas: Some(vec![0.142, 0.1425]),
        ladder: vec![LadderRung {
            n: 50,
            seed_base: 1,
            seed_count: 1,
        }],
        ..smoke_spec()
    };
    let csv = run_grid_streaming(&spec, None, None).expect("grid runs");
    assert_eq!(sim::sweep::grid_summary(&csv).len(), 2);
}

#[test]
fn a_ladder_whose_seeds_overflow_u64_is_rejected() {
    let spec = GridSpec {
        ladder: vec![LadderRung {
            n: 50,
            seed_base: u64::MAX - 1,
            seed_count: 3,
        }],
        ..smoke_spec()
    };
    assert!(expand(&spec).is_err());
}
