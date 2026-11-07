use sim::smr::ClientModel;
use sim::spec::{InjectionSpec, ProtoSpec, SmrScenarioSpec, TrafficSpec};
use sim::sweep::{LadderRung, SmrGridSpec, expand_smr, run_smr_grid_streaming};

fn base(proto: ProtoSpec, traffic: Option<TrafficSpec>) -> SmrScenarioSpec {
    SmrScenarioSpec {
        n: 64,
        seed: 0,
        k: 6,
        ell: 3,
        sigma: 1.0,
        proto,
        injections: vec![],
        max_rounds: 40,
        schedule: Some(sim::spec::ScheduleSpec::FreshPerRound { fraction: 0.0 }),
        traffic,
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

fn rate_spec() -> SmrGridSpec {
    SmrGridSpec {
        exp: "smr-smoke".into(),
        base: base(ProtoSpec::Extended, None),
        pairs: None,
        betas: Some(vec![0.0, 0.1]),
        sigmas: None,
        rates: Some(vec![1, 4]),
        t_commits: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 64,
            seed_base: 424242,
            seed_count: 2,
        }],
    }
}

#[test]
fn expand_smr_builds_point_mass_traffic_and_orders_points() {
    let points = expand_smr(&rate_spec()).expect("valid spec");
    assert_eq!(points.len(), 2 * 2 * 2); // betas x rates x seeds
    // fixed nesting: pairs -> ladder -> betas -> sigmas -> rates -> t_commits -> seeds
    let k = &points[0].0;
    assert_eq!((k.beta, k.rate, k.seed), (Some(0.0), Some(1), 424242));
    let k1 = &points[1].0;
    assert_eq!((k1.beta, k1.rate, k1.seed), (Some(0.0), Some(1), 424243));
    let k2 = &points[2].0;
    assert_eq!((k2.beta, k2.rate, k2.seed), (Some(0.0), Some(4), 424242));
    // rate r becomes a point-mass pmf with mass at index r
    let traffic = points[2].1.traffic.as_ref().expect("traffic set");
    assert_eq!(traffic.len(), 1);
    assert_eq!(traffic[0].from_round, 1);
    assert_eq!(traffic[0].arrivals_pmf, vec![0.0, 0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn rates_axis_rejects_base_traffic() {
    let spec = SmrGridSpec {
        base: base(
            ProtoSpec::Extended,
            Some(TrafficSpec::Pmf {
                arrivals_pmf: vec![1.0],
            }),
        ),
        ..rate_spec()
    };
    assert!(
        expand_smr(&spec).is_err(),
        "rates axis over an explicit base traffic is ambiguous"
    );
}

#[test]
fn t_commits_axis_requires_compact_proto() {
    let spec = SmrGridSpec {
        exp: "t".into(),
        base: base(ProtoSpec::Extended, None),
        pairs: None,
        betas: None,
        sigmas: None,
        rates: None,
        t_commits: Some(vec![12, 24]),
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 64,
            seed_base: 0,
            seed_count: 1,
        }],
    };
    assert!(expand_smr(&spec).is_err(), "t_commits needs compact proto");

    let ok = SmrGridSpec {
        base: base(ProtoSpec::Compact { t_commit_rounds: 1 }, None),
        ..spec
    };
    let points = expand_smr(&ok).expect("compact base accepts t_commits");
    assert_eq!(points.len(), 2);
    match points[1].1.proto {
        sim::smr::Proto::Compact { t_commit_rounds } => assert_eq!(t_commit_rounds, 24),
        _ => panic!("expected compact"),
    }
}

#[test]
fn smr_parallel_output_is_byte_identical_to_single_thread() {
    let dir = std::env::temp_dir().join(format!("smr-grid-par-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let spec = rate_spec();
    let single = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .expect("pool")
        .install(|| run_smr_grid_streaming(&spec, Some(&ledger), None).expect("grid runs"));
    let parallel = run_smr_grid_streaming(&spec, Some(&ledger), None).expect("grid runs");
    assert_eq!(single, parallel, "CSV must not depend on thread scheduling");
    std::fs::remove_dir_all(&dir).ok();
}

fn single_point_spec(max_rounds: usize) -> SmrGridSpec {
    let mut b = base(ProtoSpec::Extended, None);
    b.max_rounds = max_rounds;
    SmrGridSpec {
        exp: "smr-single".into(),
        base: b,
        pairs: None,
        betas: None,
        sigmas: None,
        rates: Some(vec![1]),
        t_commits: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 64,
            seed_base: 424242,
            seed_count: 1,
        }],
    }
}

/// Column index by NAME: a new key column must never silently shift a test.
fn at(name: &str) -> usize {
    sim::sweep::SMR_CSV_HEADER
        .split(',')
        .position(|c| c == name)
        .unwrap_or_else(|| panic!("no {name} column"))
}

fn col<'a, S: AsRef<str>>(cols: &'a [S], name: &str) -> &'a str {
    cols[at(name)].as_ref()
}

#[test]
fn typed_recovery_failure_is_a_prefix_abort_row_with_ledger_diagnostics() {
    let mut b = base(
        ProtoSpec::Recovery {
            t_window_rounds: 20,
            resend_until_acked: true,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        Some(TrafficSpec::Pmf {
            arrivals_pmf: vec![0.0, 0.0, 1.0],
        }),
    );
    b.n = 598;
    b.seed = 1;
    b.max_rounds = 200;
    b.schedule = Some(sim::spec::ScheduleSpec::FreshPerRound { fraction: 0.1 });
    b.certs = true;
    b.injections = vec![
        InjectionSpec {
            round: 2,
            client: 1,
            op: 1,
            target: None,
        },
        InjectionSpec {
            round: 2,
            client: 2,
            op: 2,
            target: None,
        },
    ];
    let spec = SmrGridSpec {
        exp: "typed-failure".into(),
        base: b,
        pairs: None,
        betas: None,
        sigmas: None,
        rates: None,
        t_commits: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 598,
            seed_base: 1,
            seed_count: 1,
        }],
    };
    let dir = std::env::temp_dir().join(format!("smr-grid-typed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let csv = run_smr_grid_streaming(&spec, Some(&ledger), None).expect("grid continues");
    let cols: Vec<&str> = csv.lines().nth(1).expect("row").split(',').collect();
    assert_eq!(col(&cols, "terminal"), "abort");
    let attempted: u64 = col(&cols, "rounds").parse().expect("attempted round");
    assert!((20..=200).contains(&attempted));
    assert_eq!(attempted % 20, 0);
    assert_eq!(col(&cols, "abort_kind"), "prefix");
    for name in [
        "commands",
        "complete",
        "pending",
        "lat_mean",
        "safety",
        "recovered_round",
    ] {
        assert_eq!(col(&cols, name), "", "{name} must not score a partial run");
    }
    let row: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&ledger)
            .expect("ledger")
            .lines()
            .next()
            .expect("ledger row"),
    )
    .expect("json");
    let terminal = &row["outcome"]["terminal"]["Failed"];
    let failure = &row["outcome"]["failure"];
    for diagnostic in [terminal, failure] {
        assert_eq!(diagnostic["attempted_round"], attempted);
        assert_eq!(diagnostic["completed_round"], attempted - 1);
        assert_eq!(diagnostic["observed_round"], attempted);
        assert_eq!(diagnostic["phase"], "BoundaryPreflight");
    }
    assert_eq!(failure["node"], 1);
    assert_eq!(failure["violation"]["pre_len"], 39);
    assert_eq!(failure["violation"]["log_len"], 114);
    assert_eq!(failure["violation"]["first_mismatch"], 33);
    assert_ne!(
        failure["violation"]["expected"]["entry"],
        failure["violation"]["actual"]["entry"]
    );
    std::fs::remove_dir_all(dir).ok();
}

fn grid_row_cols(spec: &SmrGridSpec, dir_tag: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("smr-grid-{dir_tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let csv =
        run_smr_grid_streaming(spec, Some(&dir.join("ledger.jsonl")), None).expect("grid runs");
    let row = csv.lines().nth(1).expect("one data row").to_string();
    std::fs::remove_dir_all(&dir).ok();
    row.split(',').map(str::to_owned).collect()
}

#[test]
fn csv_row_fields_match_report() {
    use sim::smr::{CommandStatus, SmrTerminal, run_smr};
    let spec = single_point_spec(120);
    let (_, scenario) = expand_smr(&spec).expect("valid spec").remove(0);
    let report = run_smr(&scenario);
    let cols = grid_row_cols(&spec, "match");

    let count = |s: CommandStatus| {
        report
            .commands
            .iter()
            .filter(|c| c.status == s)
            .count()
            .to_string()
    };
    let SmrTerminal::Ran { rounds } = report.terminal else {
        panic!("unblocked run must not die");
    };
    assert_eq!(
        (col(&cols, "terminal"), col(&cols, "rounds")),
        ("ran", "120")
    );
    assert_eq!(rounds, 120);
    assert_eq!(col(&cols, "commands"), report.commands.len().to_string());
    assert_eq!(col(&cols, "complete"), count(CommandStatus::Complete));
    assert_eq!(col(&cols, "pending"), count(CommandStatus::Pending));
    assert_eq!(col(&cols, "dead"), count(CommandStatus::Dead));
    assert_eq!(col(&cols, "safety"), report.safety_ok.to_string());

    let paper: Vec<(usize, Option<usize>)> = report
        .commands
        .iter()
        .filter_map(|c| c.injected_round.map(|d| (d, c.prefix_fixed_round)))
        .collect();
    let (ages, _) = windowed(&paper, 120);
    assert!(!ages.is_empty(), "120 rounds must complete some commands");
    let mean = ages.iter().sum::<usize>() as f64 / ages.len() as f64;
    assert_eq!(col(&cols, "lat_mean"), format!("{mean:.2}"));
    assert_eq!(col(&cols, "lat_end"), "prefix_fixed");
}

#[test]
fn lat_mean_blank_without_completions_and_two_decimal_otherwise() {
    let cols = grid_row_cols(&single_point_spec(2), "blank");
    assert_ne!(
        col(&cols, "commands"),
        "0",
        "traffic injected commands even in 2 rounds"
    );
    assert_eq!(col(&cols, "complete"), "0", "nothing completes in 2 rounds");
    assert_eq!(
        col(&cols, "lat_mean"),
        "",
        "no completions → empty lat_mean cell"
    );

    let cols = grid_row_cols(&single_point_spec(120), "filled");
    let (int_part, frac) = col(&cols, "lat_mean")
        .split_once('.')
        .expect("decimal point");
    assert!(!int_part.is_empty() && int_part.chars().all(|c| c.is_ascii_digit()));
    assert_eq!(frac.len(), 2, "two-decimal formatting");
    assert!(frac.chars().all(|c| c.is_ascii_digit()));
}

/// Nearest rank over the in-window population, the unsettled ranked last as `inf`.
fn nearest_rank(sorted: &[usize], total: usize, p: f64) -> String {
    let idx = (p * total as f64).ceil().max(1.0) as usize - 1;
    sorted
        .get(idx)
        .map_or_else(|| "inf".to_string(), usize::to_string)
}

/// Sorted latencies of the commands whose origin is at least the slowest observed latency before
/// the horizon, and the number of those that never settled.
fn windowed(samples: &[(usize, Option<usize>)], horizon: usize) -> (Vec<usize>, usize) {
    let slowest = samples
        .iter()
        .filter_map(|&(o, e)| e.map(|e| e - o))
        .max()
        .unwrap_or(0);
    let in_window: Vec<_> = samples
        .iter()
        .filter(|&&(o, _)| o + slowest <= horizon)
        .collect();
    let mut ages: Vec<usize> = in_window
        .iter()
        .filter_map(|&&(o, e)| e.map(|e| e - o))
        .collect();
    ages.sort_unstable();
    let unsettled = in_window.len() - ages.len();
    (ages, unsettled)
}

fn assert_latency_block(
    cols: &[String],
    prefix: &str,
    samples: &[(usize, Option<usize>)],
    horizon: usize,
) {
    let (ages, unsettled) = windowed(samples, horizon);
    assert!(!ages.is_empty());
    let name = |s: &str| format!("{prefix}{s}");
    assert_eq!(col(cols, &name("n")), ages.len().to_string(), "{prefix}n");
    assert_eq!(col(cols, &name("unsettled")), unsettled.to_string());
    let total = ages.len() + unsettled;
    for (suffix, p) in [("p50", 0.50), ("p90", 0.90), ("p99", 0.99), ("max", 1.0)] {
        assert_eq!(
            col(cols, &name(suffix)),
            nearest_rank(&ages, total, p),
            "{prefix}{suffix}"
        );
    }
}

#[test]
fn latency_quantile_cols_match_report_over_the_windowed_population() {
    use sim::smr::run_smr;
    let spec = single_point_spec(120);
    let (_, scenario) = expand_smr(&spec).expect("valid spec").remove(0);
    let report = run_smr(&scenario);
    let cols = grid_row_cols(&spec, "quant");
    let paper: Vec<_> = report
        .commands
        .iter()
        .filter_map(|c| c.injected_round.map(|d| (d, c.prefix_fixed_round)))
        .collect();
    let client: Vec<_> = report
        .commands
        .iter()
        .map(|c| (c.injection_round, c.prefix_fixed_round))
        .collect();
    assert_latency_block(&cols, "lat_", &paper, 120);
    assert_latency_block(&cols, "client_lat_", &client, 120);
}

#[test]
fn latency_window_drops_arrivals_within_the_slowest_latency_of_the_horizon() {
    use sim::smr::run_smr;
    let mut b = base(
        ProtoSpec::Compact {
            t_commit_rounds: 20,
        },
        None,
    );
    b.max_rounds = 300;
    let spec = SmrGridSpec {
        exp: "smr-window".into(),
        base: b,
        pairs: None,
        betas: None,
        sigmas: None,
        rates: Some(vec![1]),
        t_commits: Some(vec![20]),
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 64,
            seed_base: 424243,
            seed_count: 1,
        }],
    };
    let (_, scenario) = expand_smr(&spec).expect("valid spec").remove(0);
    let report = run_smr(&scenario);
    let cols = grid_row_cols(&spec, "window");
    let paper: Vec<_> = report
        .commands
        .iter()
        .filter_map(|c| c.injected_round.map(|d| (d, c.committed_round)))
        .collect();
    let client: Vec<_> = report
        .commands
        .iter()
        .map(|c| (c.injection_round, c.committed_ack_round))
        .collect();
    assert!(
        windowed(&paper, 300).0.len() < paper.len(),
        "the window must drop late arrivals"
    );
    assert_latency_block(&cols, "lat_", &paper, 300);
    assert_latency_block(&cols, "client_lat_", &client, 300);
    assert_eq!(col(&cols, "lat_end"), "committed");
}

#[test]
fn smr_streaming_partial_file_holds_rows_and_final_csv_is_byte_identical() {
    use sim::sweep::run_smr_grid_streaming;
    let dir = std::env::temp_dir().join(format!("smr-grid-stream-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let partial = dir.join("out.csv.partial");
    let _ = std::fs::remove_file(&partial);
    let spec = rate_spec();

    let reference = run_smr_grid_streaming(&spec, Some(&ledger), None).expect("grid runs");
    let streamed = run_smr_grid_streaming(&spec, Some(&ledger), Some(&partial)).expect("grid runs");

    assert_eq!(streamed, reference, "streaming must not change the CSV");
    let on_disk = std::fs::read_to_string(&partial).expect("partial written");
    // partial carries rows only (no header) — crash-surviving row store
    let rows = reference.split_once('\n').map(|(_, r)| r).unwrap();
    assert_eq!(on_disk, rows);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn smr_streaming_appends_to_existing_partial_without_header() {
    use sim::sweep::run_smr_grid_streaming;
    let dir = std::env::temp_dir().join(format!("smr-grid-append-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let partial = dir.join("out.csv.partial");
    std::fs::write(&partial, "preexisting\n").expect("seed file");
    let spec = rate_spec();

    let csv = run_smr_grid_streaming(&spec, Some(&ledger), Some(&partial)).expect("grid runs");
    let on_disk = std::fs::read_to_string(&partial).expect("partial");
    let rows = csv.split_once('\n').map(|(_, r)| r).unwrap();
    assert_eq!(on_disk, format!("preexisting\n{rows}"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn smr_grid_writes_summary_rows_and_ledger() {
    let dir = std::env::temp_dir().join(format!("smr-grid-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let _ = std::fs::remove_file(&ledger);

    let csv = run_smr_grid_streaming(&rate_spec(), Some(&ledger), None).expect("grid runs");
    let rows: Vec<&str> = csv.lines().collect();
    assert_eq!(rows[0], sim::sweep::SMR_CSV_HEADER);
    assert_eq!(rows.len(), 1 + 8);
    let header_cols = rows[0].split(',').count();
    assert_eq!(
        rows[0].split(',').next_back(),
        Some("halt_kind"),
        "outcome columns extend append-only"
    );
    for row in &rows[1..] {
        let cols: Vec<&str> = row.split(',').collect();
        assert_eq!(cols.len(), header_cols, "row must align with the header");
        assert_eq!(col(&cols, "terminal"), "ran");
        let (commands, complete, pending, dead): (usize, usize, usize, usize) = (
            col(&cols, "commands").parse().unwrap(),
            col(&cols, "complete").parse().unwrap(),
            col(&cols, "pending").parse().unwrap(),
            col(&cols, "dead").parse().unwrap(),
        );
        assert_eq!(commands, complete + pending + dead);
        assert!(commands > 0, "traffic must have injected commands");
        let safety: bool = col(&cols, "safety").parse().expect("safety parses as bool");
        assert!(safety, "unblocked smoke grid must stay safe");
    }

    let ledger_lines = std::fs::read_to_string(&ledger).expect("ledger written");
    assert_eq!(ledger_lines.lines().count(), 8);
    assert!(ledger_lines.contains("\"exp\":\"smr-smoke\""));
    std::fs::remove_dir_all(&dir).ok();
}

/// n=32, T=10, sigma=2 under `[0.4, 0.6]` client load: seed 4 trips the
/// recovery boundary-strip assert at round 40, seed 3 completes.
fn loaded_recovery_spec(seed_base: u64, seed_count: u64) -> SmrGridSpec {
    let mut b = base(
        ProtoSpec::Recovery {
            t_window_rounds: 10,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        Some(TrafficSpec::Pmf {
            arrivals_pmf: vec![0.4, 0.6],
        }),
    );
    b.sigma = 2.0;
    b.max_rounds = 90;
    SmrGridSpec {
        exp: "smr-abort".into(),
        base: b,
        pairs: None,
        betas: None,
        sigmas: None,
        rates: None,
        t_commits: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 32,
            seed_base,
            seed_count,
        }],
    }
}

#[test]
fn strip_abort_lands_as_a_row_instead_of_killing_the_sweep() {
    let cols = grid_row_cols(&loaded_recovery_spec(4, 1), "abort");
    assert_eq!(
        cols.len(),
        sim::sweep::SMR_CSV_HEADER.split(',').count(),
        "abort row must align with the header"
    );
    assert_eq!(col(&cols, "terminal"), "abort");
    assert_eq!(
        col(&cols, "rounds"),
        "40",
        "round parsed out of the assert message"
    );
    assert_eq!(col(&cols, "abort_kind"), "prefix");
    let abort_kind_at = at("abort_kind");
    for (i, cell) in cols.iter().enumerate().skip(at("commands")) {
        if i == abort_kind_at {
            continue;
        }
        assert_eq!(cell, "", "no report ⇒ column {i} stays empty");
    }
}

#[test]
fn one_abort_does_not_take_down_the_rest_of_the_grid() {
    let dir = std::env::temp_dir().join(format!("smr-grid-mixed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let _ = std::fs::remove_file(&ledger);

    // seeds 3 (runs), 4 (fails), then 7 (runs) — expansion order is preserved.
    let mut spec = loaded_recovery_spec(3, 1);
    spec.ladder = [3, 4, 7]
        .into_iter()
        .map(|seed_base| LadderRung {
            n: 32,
            seed_base,
            seed_count: 1,
        })
        .collect();
    let csv = run_smr_grid_streaming(&spec, Some(&ledger), None).expect("grid runs");
    let rows: Vec<&str> = csv.lines().skip(1).collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].split(',').nth(at("terminal")), Some("ran"));
    assert_eq!(rows[1].split(',').nth(at("terminal")), Some("abort"));
    assert_eq!(rows[2].split(',').nth(at("terminal")), Some("ran"));

    let ledger_lines = std::fs::read_to_string(&ledger).expect("ledger written");
    assert_eq!(
        ledger_lines.lines().count(),
        3,
        "failure and later run are logged"
    );
    let failed: serde_json::Value =
        serde_json::from_str(ledger_lines.lines().nth(1).unwrap()).expect("failure ledger row");
    assert_eq!(
        failed["outcome"]["terminal"]["Failed"]["attempted_round"],
        40
    );
    assert_eq!(failed["outcome"]["failure"]["phase"], "BoundaryPreflight");
    let after: serde_json::Value = serde_json::from_str(ledger_lines.lines().nth(2).unwrap())
        .expect("post-failure ledger row");
    assert_eq!(after["outcome"]["terminal"]["Ran"]["rounds"], 90);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn recovery_rows_fill_the_outcome_columns_and_other_protos_leave_them_empty() {
    let dir = std::env::temp_dir().join(format!("smr-grid-rec-{}", std::process::id()));
    let ledger = dir.join("ledger.jsonl");
    let grid = |proto: ProtoSpec, exp: &str| SmrGridSpec {
        exp: exp.into(),
        base: base(proto, None),
        pairs: None,
        betas: None,
        sigmas: None,
        rates: None,
        t_commits: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 32,
            seed_base: 7,
            seed_count: 1,
        }],
    };

    let csv = run_smr_grid_streaming(
        &grid(
            ProtoSpec::Recovery {
                t_window_rounds: 10,
                resend_until_acked: false,
                prefix_mismatch: sim::smr::PrefixMismatch::Abort,
            },
            "rec-cols",
        ),
        Some(&ledger),
        None,
    )
    .expect("grid runs");
    let header = csv.lines().next().unwrap().to_string();
    assert!(
        header.ends_with("safety,recovered_round,fork_ok,rollbacks_total,lat_end,lat_n,lat_unsettled,lat_p50,lat_p90,lat_p99,lat_max,client_lat_mean,client_lat_n,client_lat_unsettled,client_lat_p50,client_lat_p90,client_lat_p99,client_lat_max,abort_kind,strong_safety_violation_rounds,strong_safety_first_violation,strong_safety_boundary_violations,strong_safety_boundary_first_violation,retractions,retraction_rewrites,first_retraction_round,commit_fork_round,min_useful,useful_ge_n4,halt_round,halt_kind"),
        "append-only columns: {header}"
    );
    let cell = |row: &str, name: &str| -> String {
        let i = header.split(',').position(|h| h == name).expect("column");
        row.split(',').nth(i).unwrap().to_string()
    };
    let row = csv.lines().nth(1).expect("one data row").to_string();
    // Benign β = 0 run: every boundary is clean and converged, so the first
    // boundary (round 10) is the recovered round; nobody ever rolls back.
    assert_eq!(cell(&row, "recovered_round"), "10");
    assert_eq!(cell(&row, "fork_ok"), "true");
    assert_eq!(cell(&row, "rollbacks_total"), "0");

    let csv = run_smr_grid_streaming(&grid(ProtoSpec::Extended, "ext-cols"), Some(&ledger), None)
        .expect("grid runs");
    let row = csv.lines().nth(1).expect("one data row").to_string();
    assert_eq!(cell(&row, "recovered_round"), "");
    assert_eq!(cell(&row, "fork_ok"), "");
    assert_eq!(cell(&row, "rollbacks_total"), "");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn smr_splits_axis_lands_a_contiguous_two_block_partition() {
    let spec = SmrGridSpec {
        betas: None,
        rates: None,
        splits: Some(vec![0.75]),
        ..rate_spec()
    };
    let points = expand_smr(&spec).expect("valid spec");
    assert_eq!(points.len(), 2); // one split x two seeds
    assert_eq!(points[0].0.split, Some(0.75));
    let partition = points[0].1.partition.as_ref().expect("partition set");
    assert_eq!(partition.len(), 64);
    assert_eq!(partition.iter().filter(|&&c| c == 0).count(), 48);
}

#[test]
fn smr_splits_axis_rejects_a_base_that_already_names_a_partition() {
    let mut named = base(ProtoSpec::Extended, None);
    named.partition = Some(vec![0; 64]);
    let spec = SmrGridSpec {
        base: named,
        betas: None,
        rates: None,
        splits: Some(vec![0.75]),
        ..rate_spec()
    };
    assert!(expand_smr(&spec).is_err());
}

#[test]
fn the_runner_summary_reads_the_smr_terminal_column() {
    let row = |split: &str, terminal: &str| {
        let mut cols = vec![""; sim::sweep::SMR_CSV_HEADER.split(',').count()];
        cols[0] = "t";
        cols[at("k")] = "6";
        cols[at("ell")] = "3";
        cols[at("n")] = "64";
        cols[at("split")] = split;
        cols[at("seed")] = "1";
        cols[at("terminal")] = terminal;
        cols.join(",")
    };
    let csv = format!(
        "{}\n{}\n{}\n",
        sim::sweep::SMR_CSV_HEADER,
        row("0.500", "dead"),
        row("0.750", "ran"),
    );
    let lines = sim::sweep::grid_summary(&csv);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with(": 0/1 ran"), "got {}", lines[0]);
    assert!(lines[1].ends_with(": 1/1 ran"), "got {}", lines[1]);
    assert!(lines[1].contains("split=0.750"));
}

#[test]
fn the_smr_split_column_sits_between_horizon_and_seed() {
    let cols: Vec<&str> = sim::sweep::SMR_CSV_HEADER.split(',').collect();
    let at = cols
        .iter()
        .position(|&c| c == "split")
        .expect("split column");
    assert_eq!((cols[at - 1], cols[at + 1]), ("horizon", "seed"));
}

// Seed 98 commits a pool client's sn out of order under the execute-once state machine; seed 97 does not.
#[test]
fn an_out_of_sn_order_commit_is_an_sn_order_abort_row_and_the_grid_continues() {
    let mut b = base(
        ProtoSpec::Compact { t_commit_rounds: 2 },
        Some(TrafficSpec::Pmf {
            arrivals_pmf: vec![0.0, 0.0, 1.0],
        }),
    );
    b.n = 16;
    b.max_rounds = 300;
    b.schedule = Some(sim::spec::ScheduleSpec::FreshPerRound { fraction: 0.2 });
    b.client_model = ClientModel::Pool { clients: 24 };
    b.repeated_commit = sim::smr::RepeatedCommit::Skip;
    let spec = SmrGridSpec {
        exp: "sn-order".into(),
        base: b,
        pairs: None,
        betas: None,
        sigmas: None,
        rates: None,
        t_commits: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 16,
            seed_base: 97,
            seed_count: 2,
        }],
    };
    let dir = std::env::temp_dir().join(format!("smr-grid-sn-order-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let csv = run_smr_grid_streaming(&spec, Some(&ledger), None).expect("grid continues");
    let rows: Vec<Vec<&str>> = csv
        .lines()
        .skip(1)
        .map(|l| l.split(',').collect())
        .collect();
    assert_eq!(rows.len(), 2, "one row per point");
    let failed = rows
        .iter()
        .find(|r| col(r, "seed") == "98")
        .expect("seed 98 row");
    assert_eq!(col(failed, "terminal"), "abort");
    assert_eq!(col(failed, "abort_kind"), "sn_order");
    let other = rows
        .iter()
        .find(|r| col(r, "seed") == "97")
        .expect("seed 97 row");
    assert_ne!(col(other, "terminal"), "abort");
    let failure = std::fs::read_to_string(&ledger)
        .expect("ledger")
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("json"))
        .find(|row| row["scenario"]["seed"] == 98)
        .expect("seed 98 ledger row")["outcome"]["failure"]
        .clone();
    assert_eq!(failure["phase"], "OutOfSnOrderCommit");
    assert!(failure["violation"]["committed_sn"].is_u64());
    std::fs::remove_dir_all(dir).ok();
}

// Def 1.4 / Def 1.5 columns: appended after abort_kind, filled per executing tier, blank otherwise.
#[test]
fn committed_sequence_columns_follow_the_report_and_stay_blank_where_unmeasured() {
    let mut b = base(
        ProtoSpec::Compact {
            t_commit_rounds: 10,
        },
        Some(TrafficSpec::Pmf {
            arrivals_pmf: vec![0.0, 0.0, 1.0],
        }),
    );
    b.n = 100;
    b.max_rounds = 60;
    let spec = SmrGridSpec {
        exp: "committed-cols".into(),
        base: b,
        ladder: vec![LadderRung {
            n: 100,
            seed_base: 7001,
            seed_count: 1,
        }],
        ..single_point_spec(60)
    };
    let spec = SmrGridSpec {
        rates: None,
        ..spec
    };
    let (_, scenario) = expand_smr(&spec).expect("valid").remove(0);
    let report = sim::smr::run_smr_grid(&scenario);
    assert!(report.strong_safety_violation_rounds > 0, "precondition");
    let cols = grid_row_cols(&spec, "committed-cols");
    let opt = |v: Option<usize>| v.map(|v| v.to_string()).unwrap_or_default();
    assert_eq!(
        [
            col(&cols, "strong_safety_violation_rounds"),
            col(&cols, "strong_safety_first_violation"),
            col(&cols, "strong_safety_boundary_violations"),
            col(&cols, "retractions"),
            col(&cols, "retraction_rewrites"),
            col(&cols, "first_retraction_round"),
            col(&cols, "commit_fork_round"),
        ],
        [
            report.strong_safety_violation_rounds.to_string().as_str(),
            opt(report.strong_safety_first_violation).as_str(),
            "",
            report.retractions.to_string().as_str(),
            report.retraction_rewrites.to_string().as_str(),
            opt(report.first_retraction_round).as_str(),
            "",
        ]
    );
    let ext = grid_row_cols(&single_point_spec(40), "committed-cols-ext");
    assert_eq!(ext.len(), sim::sweep::SMR_CSV_HEADER.split(',').count());
    assert_eq!(col(&ext, "retractions"), "");
    assert_eq!(col(&ext, "strong_safety_violation_rounds"), "");
}

// One pool client and an arrival every round: the second arrival finds the pool empty.
#[test]
fn an_exhausted_client_pool_is_a_pool_abort_row_and_the_grid_continues() {
    let mut b = base(
        ProtoSpec::Compact { t_commit_rounds: 5 },
        Some(TrafficSpec::Pmf {
            arrivals_pmf: vec![0.0, 1.0],
        }),
    );
    b.n = 16;
    b.client_model = ClientModel::Pool { clients: 1 };
    let spec = SmrGridSpec {
        exp: "pool".into(),
        base: b,
        pairs: None,
        betas: None,
        sigmas: None,
        rates: None,
        t_commits: None,
        horizons: Some(vec![1, 40]),
        splits: None,
        ladder: vec![LadderRung {
            n: 16,
            seed_base: 1,
            seed_count: 1,
        }],
    };
    let dir = std::env::temp_dir().join(format!("smr-grid-pool-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let csv = run_smr_grid_streaming(&spec, Some(&ledger), None).expect("grid continues");
    let rows: Vec<Vec<&str>> = csv
        .lines()
        .skip(1)
        .map(|l| l.split(',').collect())
        .collect();
    assert_eq!(rows.len(), 2, "one row per point");
    let survived = rows.iter().find(|r| col(r, "horizon") == "1").expect("row");
    assert_eq!(col(survived, "terminal"), "ran");
    let exhausted = rows
        .iter()
        .find(|r| col(r, "horizon") == "40")
        .expect("row");
    assert_eq!(col(exhausted, "terminal"), "abort");
    assert_eq!(col(exhausted, "abort_kind"), "pool");
    assert_eq!(
        col(exhausted, "rounds"),
        "2",
        "the second arrival is in round 2"
    );
    let ledger_rows: Vec<serde_json::Value> = std::fs::read_to_string(&ledger)
        .expect("ledger")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    let aborted = ledger_rows
        .iter()
        .find(|r| r["outcome"]["terminal"] == "abort")
        .expect("abort ledger row");
    assert_eq!(aborted["extra"]["abort_kind"], "pool");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn rows_flag_whether_n_over_4_servers_were_useful_in_every_round() {
    let surge = |fraction: f64, tag: &str| {
        let mut spec = single_point_spec(30);
        spec.base.schedule = Some(sim::spec::ScheduleSpec::Windows {
            windows: vec![sim::spec::WindowSpec {
                start_round: 5,
                rounds: 3,
                target: sim::spec::TargetSpec::SampleFraction { fraction },
            }],
        });
        grid_row_cols(&spec, tag)
    };
    let calm = surge(0.0, "calm");
    assert_eq!(col(&calm, "min_useful"), "64");
    assert_eq!(col(&calm, "useful_ge_n4"), "true");
    let hit = surge(0.9, "surge");
    assert!(col(&hit, "min_useful").parse::<u32>().expect("count") < 16);
    assert_eq!(col(&hit, "useful_ge_n4"), "false");
}

// A halt is its own terminal: the row, its appended columns and the ledger row all name it.
#[test]
fn a_halted_run_is_a_halted_row_and_ledger_outcome() {
    let dir = std::env::temp_dir().join(format!("smr-grid-halt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ledger = dir.join("ledger.jsonl");
    let _ = std::fs::remove_file(&ledger);
    let mut b = base(
        ProtoSpec::Compact {
            t_commit_rounds: 21,
        },
        Some(TrafficSpec::Pmf {
            arrivals_pmf: vec![0.0, 0.0, 0.0, 0.0, 1.0],
        }),
    );
    b.max_rounds = 400;
    let spec = |halt: bool| SmrGridSpec {
        exp: format!("halt-{halt}"),
        base: SmrScenarioSpec {
            halt_on_violation: halt,
            ..b.clone()
        },
        pairs: None,
        betas: None,
        sigmas: None,
        rates: None,
        t_commits: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n: 64,
            seed_base: 950_000,
            seed_count: 1,
        }],
    };
    let row = |halt: bool| -> Vec<String> {
        let csv = run_smr_grid_streaming(&spec(halt), Some(&ledger), None).expect("grid runs");
        csv.lines()
            .nth(1)
            .unwrap()
            .split(',')
            .map(str::to_owned)
            .collect()
    };
    let (off, on) = (row(false), row(true));
    assert_eq!(
        col(&off, "safety"),
        "false",
        "precondition: the cell splits its brain"
    );
    assert_eq!(
        (
            col(&off, "terminal"),
            col(&off, "halt_round"),
            col(&off, "halt_kind")
        ),
        ("ran", "", "")
    );
    assert_eq!(col(&on, "terminal"), "halted");
    assert_eq!(col(&on, "halt_kind"), "fork");
    assert_eq!(col(&on, "halt_round"), col(&on, "rounds"));
    assert!(col(&on, "rounds").parse::<usize>().unwrap() < 400);

    let rows: Vec<serde_json::Value> = std::fs::read_to_string(&ledger)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows[0]["scenario"].get("halt_on_violation").is_none());
    assert_eq!(rows[1]["scenario"]["halt_on_violation"], true);
    let halt_round: usize = col(&on, "halt_round").parse().unwrap();
    assert_eq!(
        rows[1]["outcome"]["terminal"],
        serde_json::json!({"Halted": {"round": halt_round, "kind": "fork"}})
    );
    std::fs::remove_dir_all(&dir).ok();
}
