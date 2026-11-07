//! Latency columns: paper latency runs from the injection (first receipt by a non-blocked
//! server, pp. 7, 19) to a named end landmark; the client-centred columns are separate.

use protocol::compact::Entry;
use sim::smr::{AttemptOutcome, ClientModel, CommandStatus, SmrState, run_smr};
use sim::spec::{InjectionSpec, ProtoSpec, ScheduleSpec, SmrScenarioSpec, TargetSpec, WindowSpec};
use sim::sweep::{LadderRung, SmrGridSpec, expand_smr, run_smr_grid_streaming};
use std::collections::HashMap;

fn spec_of(
    proto: ProtoSpec,
    n: usize,
    injections: Vec<InjectionSpec>,
    schedule: ScheduleSpec,
    max_rounds: usize,
) -> SmrScenarioSpec {
    SmrScenarioSpec {
        n,
        seed: 0,
        k: 6,
        ell: 3,
        sigma: 1.0,
        proto,
        injections,
        max_rounds,
        schedule: Some(schedule),
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

fn one_row_grid(
    base: SmrScenarioSpec,
    betas: Option<Vec<f64>>,
    rates: Option<Vec<usize>>,
    seed: u64,
) -> SmrGridSpec {
    let n = base.n;
    SmrGridSpec {
        exp: "latency-origin".into(),
        base,
        pairs: None,
        betas,
        sigmas: None,
        rates,
        t_commits: None,
        horizons: None,
        splits: None,
        ladder: vec![LadderRung {
            n,
            seed_base: seed,
            seed_count: 1,
        }],
    }
}

fn grid_row(spec: &SmrGridSpec, tag: &str) -> HashMap<String, String> {
    let dir = std::env::temp_dir().join(format!("smr-latency-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tempdir");
    let csv =
        run_smr_grid_streaming(spec, Some(&dir.join("ledger.jsonl")), None).expect("grid runs");
    std::fs::remove_dir_all(&dir).ok();
    let mut lines = csv.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    let row: Vec<&str> = lines.next().expect("one data row").split(',').collect();
    header
        .iter()
        .zip(row)
        .map(|(h, v)| (h.to_string(), v.to_string()))
        .collect()
}

fn nearest_rank(sorted: &[usize], p: f64) -> usize {
    let idx = (p * sorted.len() as f64).ceil().max(1.0) as usize - 1;
    sorted[idx]
}

fn window(start_round: usize, rounds: usize, ids: Vec<u32>) -> WindowSpec {
    WindowSpec {
        start_round,
        rounds,
        target: TargetSpec::Nodes { ids },
    }
}

fn compact(t: u64) -> ProtoSpec {
    ProtoSpec::Compact { t_commit_rounds: t }
}

fn inj(round: usize, client: u32, op: u64, target: Option<u32>) -> InjectionSpec {
    InjectionSpec {
        round,
        client,
        op,
        target,
    }
}

#[test]
fn header_renames_every_latency_column_whose_meaning_changed() {
    let header: Vec<&str> = sim::sweep::SMR_CSV_HEADER.split(',').collect();
    for old in [
        "mean_te",
        "te_n",
        "te_unsettled",
        "te_p50",
        "te_p90",
        "te_p99",
        "te_max",
    ] {
        assert!(!header.contains(&old), "{old} must not survive the rename");
    }
    for new in [
        "lat_end",
        "lat_mean",
        "lat_n",
        "lat_unsettled",
        "lat_p50",
        "lat_p90",
        "lat_p99",
        "lat_max",
        "client_lat_mean",
        "client_lat_n",
        "client_lat_unsettled",
        "client_lat_p50",
        "client_lat_p90",
        "client_lat_p99",
        "client_lat_max",
    ] {
        assert!(header.contains(&new), "missing column {new}");
    }
}

// pp. 7, 19: the target is blocked in round 2, so the injection is round 3.
#[test]
fn paper_latency_counts_from_first_non_blocked_receipt() {
    let base = spec_of(
        ProtoSpec::Extended,
        4,
        vec![inj(2, 1, 7, Some(0))],
        ScheduleSpec::Windows {
            windows: vec![window(2, 1, vec![0])],
        },
        40,
    );
    let spec = one_row_grid(base, None, None, 0);
    let (_, scenario) = expand_smr(&spec).expect("valid spec").remove(0);
    let report = run_smr(&scenario);
    let cmd = &report.commands[0];
    assert_eq!(cmd.injected_round, Some(3));
    let fixed = cmd
        .prefix_fixed_round
        .expect("prefix fixed within 40 rounds");
    let row = grid_row(&spec, "mech");
    assert_eq!(row["lat_end"], "prefix_fixed");
    assert_eq!(
        (row["lat_mean"].clone(), row["lat_max"].clone()),
        (
            format!("{:.2}", (fixed - 3) as f64),
            (fixed - 3).to_string()
        )
    );
    assert_eq!(
        (
            row["client_lat_mean"].clone(),
            row["client_lat_max"].clone()
        ),
        (
            format!("{:.2}", (fixed - 2) as f64),
            (fixed - 2).to_string()
        ),
        "the client-centred columns keep counting from the scheduled attempt"
    );
}

#[test]
fn paper_latency_quantiles_match_schedule_derived_injection_rounds() {
    let windows = vec![
        window(2, 3, vec![0]),
        window(3, 1, vec![1]),
        window(5, 2, vec![2]),
    ];
    let blocked_in = |node: u32, r: usize| {
        windows.iter().any(|w| {
            r >= w.start_round
                && r < w.start_round + w.rounds
                && matches!(&w.target, TargetSpec::Nodes { ids } if ids.contains(&node))
        })
    };
    let injections = vec![
        inj(2, 1, 11, Some(0)),
        inj(3, 2, 12, Some(1)),
        inj(5, 3, 13, Some(2)),
        inj(6, 4, 14, Some(3)),
    ];
    let injected: Vec<(u64, usize)> = injections
        .iter()
        .map(|i| {
            let target = i.target.unwrap();
            (
                (i.op),
                (i.round..).find(|&r| !blocked_in(target, r)).unwrap(),
            )
        })
        .collect();
    let base = spec_of(
        ProtoSpec::Extended,
        8,
        injections,
        ScheduleSpec::Windows { windows },
        60,
    );
    let spec = one_row_grid(base, None, None, 0);
    let (_, scenario) = expand_smr(&spec).expect("valid spec").remove(0);
    let report = run_smr(&scenario);
    let mut ages: Vec<usize> = report
        .commands
        .iter()
        .map(|c| {
            let inj = injected.iter().find(|(op, _)| *op == c.op).unwrap().1;
            assert_eq!(c.injected_round, Some(inj), "setup: op {}", c.op);
            c.prefix_fixed_round.unwrap() - inj
        })
        .collect();
    ages.sort_unstable();
    let mean = ages.iter().sum::<usize>() as f64 / ages.len() as f64;
    let row = grid_row(&spec, "diff");
    assert_eq!(
        (
            row["lat_mean"].clone(),
            row["lat_p50"].clone(),
            row["lat_p90"].clone(),
            row["lat_max"].clone()
        ),
        (
            format!("{mean:.2}"),
            nearest_rank(&ages, 0.5).to_string(),
            nearest_rank(&ages, 0.9).to_string(),
            ages[3].to_string()
        )
    );
}

// p. 19: under fresh blocking some first attempts hit a blocked server and inject later.
#[test]
fn paper_latency_under_fresh_blocking_counts_from_injection() {
    let base = spec_of(
        ProtoSpec::Extended,
        256,
        vec![],
        ScheduleSpec::FreshPerRound { fraction: 0.0 },
        120,
    );
    let spec = one_row_grid(base, Some(vec![0.1]), Some(vec![4]), 424242);
    let (_, scenario) = expand_smr(&spec).expect("valid spec").remove(0);
    let report = run_smr(&scenario);
    let late = report
        .commands
        .iter()
        .filter(|c| c.injected_round.is_some_and(|d| d > c.injection_round))
        .count();
    assert!(late > 0, "beta = 0.1 must delay some first attempts");
    let row = grid_row(&spec, "scen");
    let cutoff_window = |origin: fn(&sim::smr::CommandReport) -> usize| {
        let ages: Vec<(usize, usize)> = report
            .commands
            .iter()
            .filter(|c| c.status == CommandStatus::Complete && c.injected_round.is_some())
            .map(|c| (origin(c), c.prefix_fixed_round.unwrap() - origin(c)))
            .collect();
        let w = ages.iter().map(|a| a.1).max().unwrap();
        let kept: Vec<usize> = ages
            .iter()
            .filter(|a| a.0 + w <= 120)
            .map(|a| a.1)
            .collect();
        format!(
            "{:.2}",
            kept.iter().sum::<usize>() as f64 / kept.len() as f64
        )
    };
    assert_eq!(
        row["lat_mean"],
        cutoff_window(|c| c.injected_round.unwrap())
    );
    assert_eq!(row["client_lat_mean"], cutoff_window(|c| c.injection_round));
    assert_ne!(row["lat_mean"], row["client_lat_mean"]);
}

/// Replays the scenario under `masks`; returns (injected_round, first round every server executed the command).
fn replay_commit(spec: &SmrGridSpec, masks: &dyn Fn(usize) -> Vec<bool>) -> (usize, usize) {
    let (_, sc) = expand_smr(spec).expect("valid").remove(0);
    let mut state = SmrState::new_with_certs(
        sc.n,
        sc.cfg,
        sc.proto,
        sc.sigma,
        sc.seed,
        &sc.injections,
        sc.traffic.as_deref().unwrap_or(&[]),
        sc.client_model,
        sc.partition.as_deref(),
        false,
        sc.merge_policy,
        sc.repeated_commit,
    );
    let mut all_executed = None;
    for round in 1..=sc.max_rounds {
        state.step_masked(&masks(round));
        let has = |i: usize| {
            state
                .executed_entries(i)
                .expect("compact")
                .iter()
                .any(|e| matches!(e, Entry::Cmd(c) if c.client == 1 && c.op == 7))
        };
        if all_executed.is_none() && (0..sc.n).all(has) {
            all_executed = Some(round);
        }
    }
    let report = state.report();
    let injected = report.commands[0].injected_round.expect("injected");
    assert_eq!(
        report.commands[0].committed_round, all_executed,
        "committed_round is the first round every server executed the command"
    );
    (injected, all_executed.expect("committed on every server"))
}

fn compact_one_command(schedule: ScheduleSpec) -> SmrGridSpec {
    let mut base = spec_of(compact(10), 8, vec![inj(1, 1, 7, Some(0))], schedule, 60);
    base.sigma = 2.0;
    one_row_grid(base, None, None, 0)
}

// M.42, p. 5: compact latency ends at commitment on all servers, not at the later client ack.
#[test]
fn compact_latency_ends_at_commitment_not_at_the_client_ack() {
    let spec = compact_one_command(ScheduleSpec::FreshPerRound { fraction: 0.0 });
    let (injected, committed) = replay_commit(&spec, &|_| vec![false; 8]);
    let row = grid_row(&spec, "comp-end");
    assert_eq!(row["lat_end"], "committed");
    assert_eq!(row["lat_max"], (committed - injected).to_string());
    let (_, sc) = expand_smr(&spec).expect("valid").remove(0);
    let ack = run_smr(&sc).commands[0].committed_ack_round.expect("acked");
    assert!(
        ack > committed,
        "the client learns the commit after it happened"
    );
    assert_eq!(row["client_lat_max"], (ack - 1).to_string());
}

#[test]
fn compact_latency_starts_at_first_non_blocked_receipt() {
    let spec = compact_one_command(ScheduleSpec::Windows {
        windows: vec![window(1, 3, vec![0])],
    });
    let masks = |r: usize| (0..8).map(|i| i == 0 && r <= 3).collect::<Vec<bool>>();
    let (injected, committed) = replay_commit(&spec, &masks);
    assert_eq!(injected, 4, "the first round server 0 is not blocked");
    let row = grid_row(&spec, "comp-start");
    assert_eq!(row["lat_max"], (committed - injected).to_string());
}

#[test]
fn compact_rows_report_mean_latency() {
    let spec = compact_one_command(ScheduleSpec::FreshPerRound { fraction: 0.0 });
    let row = grid_row(&spec, "comp-mean");
    assert_eq!(
        row["lat_mean"],
        format!("{:.2}", row["lat_p50"].parse::<f64>().unwrap())
    );
    assert!(!row["client_lat_mean"].is_empty());
}

fn extended_rate_grid(n: usize, rate: usize, horizon: usize) -> SmrGridSpec {
    let base = spec_of(
        ProtoSpec::Extended,
        n,
        vec![],
        ScheduleSpec::FreshPerRound { fraction: 0.0 },
        horizon,
    );
    one_row_grid(base, None, Some(vec![rate]), 5)
}

// Commands injected within the slowest observed latency of the horizon cannot have settled yet.
#[test]
fn extended_statistics_exclude_commands_injected_near_the_horizon() {
    let row = grid_row(&extended_rate_grid(64, 4, 80), "ext-window");
    assert_eq!(row["lat_unsettled"], "0");
    assert_eq!(row["client_lat_unsettled"], "0");
}

#[test]
fn extended_mean_matches_the_windowed_population() {
    let spec = extended_rate_grid(64, 4, 80);
    let (_, sc) = expand_smr(&spec).expect("valid").remove(0);
    let report = run_smr(&sc);
    let ages: Vec<(usize, usize)> = report
        .commands
        .iter()
        .filter(|c| c.status == CommandStatus::Complete)
        .map(|c| {
            let d = c.injected_round.unwrap();
            (d, c.prefix_fixed_round.unwrap() - d)
        })
        .collect();
    let w = ages.iter().map(|a| a.1).max().unwrap();
    let windowed: Vec<usize> = ages.iter().filter(|a| a.0 + w <= 80).map(|a| a.1).collect();
    let mean = windowed.iter().sum::<usize>() as f64 / windowed.len() as f64;
    let row = grid_row(&spec, "ext-mean");
    assert_eq!(row["lat_mean"], format!("{mean:.2}"));
    assert_eq!(row["lat_n"], windowed.len().to_string());
}

fn recovery(t: u64, resend_until_acked: bool) -> ProtoSpec {
    ProtoSpec::Recovery {
        t_window_rounds: t,
        resend_until_acked,
        prefix_mismatch: Default::default(),
    }
}

/// One client, two commands at round 1 to server 0: the second (sn 2) is received but ignored
/// until the first commits (Alg 5 "sn(x) = sn(c) + 1"), so it is injected long before it amplifies.
fn sn_gapped(proto: ProtoSpec) -> SmrGridSpec {
    let mut base = spec_of(
        proto,
        8,
        vec![inj(1, 1, 7, Some(0)), inj(1, 1, 8, Some(0))],
        ScheduleSpec::FreshPerRound { fraction: 0.0 },
        80,
    );
    base.sigma = 2.0;
    one_row_grid(base, None, None, 0)
}

fn landmarks_of(spec: &SmrGridSpec, op: u64) -> sim::smr::CommandReport {
    let (_, sc) = expand_smr(spec).expect("valid").remove(0);
    run_smr(&sc)
        .commands
        .into_iter()
        .find(|c| c.op == op)
        .expect("command reported")
}

// pp. 7, 19, read literally: injection is the first receipt by a non-blocked server, ignored or not.
#[test]
fn an_ignored_receipt_injects_the_command_in_compact_and_recovery() {
    for proto in [compact(10), recovery(8, true), recovery(8, false)] {
        let spec = sn_gapped(proto.clone());
        let first = landmarks_of(&spec, 7);
        let gapped = landmarks_of(&spec, 8);
        assert_eq!(
            (first.injected_round, first.delivered_round),
            (Some(1), Some(1)),
            "{proto:?}"
        );
        assert_eq!(gapped.injected_round, Some(1), "{proto:?}");
        assert!(
            gapped.delivered_round.expect("amplified eventually") > 1,
            "{proto:?}: the sn-gapped command amplifies only after its predecessor commits"
        );
    }
}

#[test]
fn compact_paper_latency_counts_from_the_ignored_receipt() {
    let spec = sn_gapped(compact(10));
    let end = |op| landmarks_of(&spec, op).committed_round.expect("committed");
    let row = grid_row(&spec, "sn-gap");
    assert_eq!(row["lat_end"], "committed");
    assert_eq!(row["lat_n"], "2");
    assert_eq!(row["lat_max"], (end(7).max(end(8)) - 1).to_string());
}

// Server 0, blocked in rounds 1-3, gets no replies there and holds L_0 = ⊥ in round 4: it receives
// and ignores the command (injection), and amplifies it in round 5 once it holds a log again.
#[test]
fn a_blocked_receipt_does_not_inject_and_a_bot_receipt_does() {
    let spec = compact_one_command(ScheduleSpec::Windows {
        windows: vec![window(1, 3, vec![0])],
    });
    let c = landmarks_of(&spec, 7);
    assert_eq!((c.injected_round, c.delivered_round), (Some(4), Some(5)));
    assert_eq!(
        c.attempts
            .iter()
            .map(|a| (a.round, a.outcome))
            .take(5)
            .collect::<Vec<_>>(),
        vec![
            (1, AttemptOutcome::TargetBlocked),
            (2, AttemptOutcome::TargetBlocked),
            (3, AttemptOutcome::TargetBlocked),
            (4, AttemptOutcome::Ignored),
            (5, AttemptOutcome::Amplified),
        ]
    );
}

#[test]
fn extended_injection_is_its_delivery() {
    let spec = extended_rate_grid(64, 4, 80);
    let (_, mut sc) = expand_smr(&spec).expect("valid").remove(0);
    sc.schedule = sim::BlockSchedule::FreshPerRound { fraction: 0.2 };
    let report = run_smr(&sc);
    assert!(
        report
            .commands
            .iter()
            .any(|c| c.injected_round.is_some_and(|r| r > c.injection_round))
    );
    for c in &report.commands {
        assert_eq!(c.injected_round, c.delivered_round);
    }
}

// The ack client stops on the ack even though every useful server executed the command earlier.
#[test]
fn recovery_client_latency_ends_where_its_client_stops() {
    for acked in [true, false] {
        let spec = sn_gapped(recovery(8, acked));
        let rows = grid_row(&spec, if acked { "rec-ack" } else { "rec-stress" });
        let ends: Vec<usize> = [7, 8]
            .map(|op| {
                let c = landmarks_of(&spec, op);
                if acked {
                    c.committed_ack_round.expect("acked")
                } else {
                    c.executed_round.expect("executed")
                }
            })
            .to_vec();
        assert_eq!(rows["client_lat_n"], "2");
        assert_eq!(
            rows["client_lat_max"],
            (ends.iter().max().unwrap() - 1).to_string(),
            "resend_until_acked = {acked}"
        );
    }
    let c = landmarks_of(&sn_gapped(recovery(8, true)), 7);
    assert!(
        c.executed_round.unwrap() < c.committed_ack_round.unwrap(),
        "precondition: the earliest landmark is not the ack client's end"
    );
}
