use super::*;
use crate::smr::{CommandReport, CommandStatus, client_end_round, paper_latency_sample};

const STRIP_PANIC: &str =
    "P must be a prefix of L: strip would corrupt the log (pre len 7, log len 17 at round 40)";

#[test]
fn a_split_brain_row_is_identical_with_and_without_truncation() {
    let scenario = crate::smr::SmrScenario {
        n: 64,
        seed: 950_000,
        cfg: crate::Config::default(),
        sigma: 1.0,
        proto: Proto::Compact {
            t_commit_rounds: 21,
        },
        injections: vec![],
        max_rounds: 400,
        schedule: crate::BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: Some(vec![crate::smr::TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.0, 0.0, 0.0, 0.0, 1.0],
        }]),
        client_model: crate::smr::ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    };
    let key = SmrRowKey {
        k: 6,
        ell: 3,
        n: 64,
        beta: Some(0.0),
        sigma: Some(1.0),
        rate: Some(4),
        t_commit: Some(21),
        split: None,
        horizon: Some(400),
        seed: 950_000,
    };
    let grid = crate::smr::run_smr_grid(&scenario);
    let reference = crate::smr::run_smr(&scenario);
    assert!(!grid.safety_ok, "the cell must still split its brain");
    assert!(!reference.safety_ok);
    assert_eq!(
        smr_row(
            "split",
            &key,
            scenario.proto,
            &SmrSummary::from_report(&grid, &scenario)
        ),
        smr_row(
            "split",
            &key,
            scenario.proto,
            &SmrSummary::from_report(&reference, &scenario)
        )
    );
}

#[test]
fn clean_values_pass_through() {
    assert_eq!(capture_strip_abort(|| 7).expect("no panic"), 7);
}

#[test]
fn strip_abort_is_captured_with_its_round() {
    let abort = capture_strip_abort::<()>(|| panic!("{STRIP_PANIC}")).expect_err("captured");
    assert_eq!(abort.round, Some(40));
    assert_eq!(abort.kind, AbortKind::Prefix);
}

#[test]
fn strip_abort_without_a_round_still_captures() {
    let abort =
        capture_strip_abort::<()>(|| panic!("P must be a prefix of L")).expect_err("captured");
    assert_eq!(abort.round, None);
    assert_eq!(abort.kind, AbortKind::Prefix);
}

#[test]
fn pool_exhaustion_is_captured_with_its_round() {
    let abort = capture_strip_abort::<()>(|| {
        panic!("client pool exhausted in round 7: all 3 clients are in flight")
    })
    .expect_err("captured");
    assert_eq!(abort.round, Some(7));
    assert_eq!(abort.kind, AbortKind::Pool);
    assert_eq!(AbortKind::Pool.as_str(), "pool");
}

#[test]
fn fmt_opt_keeps_three_decimals_and_never_merges_finer_values() {
    assert_eq!(fmt_opt(Some(0.1)), "0.100");
    assert_eq!(fmt_opt(Some(0.142)), "0.142");
    assert_eq!(fmt_opt(None), "");
    assert_ne!(fmt_opt(Some(0.142)), fmt_opt(Some(0.1425)));
    assert_eq!(fmt_opt(Some(0.1425)), "0.1425");
}

#[test]
fn a_recorded_failure_names_its_abort_kind() {
    assert_eq!(
        AbortKind::of_failure(RunFailurePhase::BoundaryPreflight),
        AbortKind::Prefix
    );
    assert_eq!(
        AbortKind::of_failure(RunFailurePhase::ExecutedRegression),
        AbortKind::Mono
    );
    assert_eq!(
        AbortKind::of_failure(RunFailurePhase::OutOfSnOrderCommit),
        AbortKind::SnOrder
    );
    assert_eq!(AbortKind::SnOrder.as_str(), "sn_order");
}

#[test]
#[should_panic(expected = "an unrecognised engine failure")]
fn other_panics_are_not_swallowed() {
    let _ = capture_strip_abort::<()>(|| panic!("an unrecognised engine failure"));
}

fn cols(s: &LatencySummary) -> (&str, usize, usize, &str, &str, &str, &str) {
    (
        s.mean.as_str(),
        s.n,
        s.unsettled,
        s.p50.as_str(),
        s.p90.as_str(),
        s.p99.as_str(),
        s.max.as_str(),
    )
}

// Hand-computed: settled latencies 3, 10, 2, 14 → slowest 14, window origin ≤ 100 − 14 = 86, so
// origin 95 is out. In window, sorted: 2, 3, 10, 14, ∞ (origin 30 never settled). Nearest rank
// over 5: p50 → 3rd = 10, p90 → 5th = ∞, p99 → 5th = ∞, max = ∞. Mean over the settled four: 7.25.
#[test]
fn an_unsettled_command_in_the_window_is_an_infinite_latency_in_every_quantile() {
    let samples = [
        (10, Some(13)),
        (20, Some(30)),
        (30, None),
        (40, Some(42)),
        (95, None),
        (85, Some(99)),
    ];
    assert_eq!(
        cols(&summarize_latency(samples, 100)),
        ("7.25", 4, 1, "10", "inf", "inf", "inf")
    );
}

// Settled 4, 1, 6, 2, 9 → slowest 9, window origin ≤ 41: origin 45 is out. Sorted 1, 2, 4, 6, ∞, ∞
// over 6: p50 → 3rd = 4, p90 → 6th = ∞. Mean (1 + 2 + 4 + 6) / 4 = 3.25.
#[test]
fn the_window_drops_late_origins_before_censoring() {
    let samples = [
        (1, Some(5)),
        (2, Some(3)),
        (3, Some(9)),
        (4, None),
        (5, Some(7)),
        (40, None),
        (45, Some(54)),
    ];
    assert_eq!(
        cols(&summarize_latency(samples, 50)),
        ("3.25", 4, 2, "4", "inf", "inf", "inf")
    );
}

#[test]
fn without_censoring_the_quantiles_are_finite() {
    let samples = [(1, Some(3)), (2, Some(3)), (3, Some(7))];
    assert_eq!(
        cols(&summarize_latency(samples, 20)),
        ("2.33", 3, 0, "2", "4", "4", "4")
    );
}

#[test]
fn nothing_settled_reports_no_mean_and_infinite_quantiles() {
    assert_eq!(
        cols(&summarize_latency([(3, None)], 10)),
        ("", 0, 1, "inf", "inf", "inf", "inf")
    );
    assert_eq!(cols(&summarize_latency([], 10)), ("", 0, 0, "", "", "", ""));
}

#[test]
#[should_panic(expected = "precedes")]
fn a_landmark_before_its_origin_is_a_bug() {
    summarize_latency([(5, Some(4))], 10);
}

fn command() -> CommandReport {
    CommandReport {
        client: 1,
        op: 7,
        injection_round: 2,
        injected_round: Some(2),
        delivered_round: Some(2),
        all_logs_round: Some(8),
        prefix_fixed_round: Some(12),
        index_agreed_round: None,
        committed_ack_round: Some(30),
        committed_round: Some(25),
        executed_round: Some(24),
        auto: false,
        spread: crate::smr::SpreadCurve::NotObserved,
        attempts: Vec::new(),
        amp_receivers: Vec::new(),
        status: CommandStatus::Complete,
    }
}

#[test]
fn the_client_end_landmark_is_where_the_proto_client_stops() {
    let c = command();
    let recovery = |resend_until_acked| Proto::Recovery {
        t_window_rounds: 8,
        resend_until_acked,
        prefix_mismatch: crate::smr::PrefixMismatch::Abort,
    };
    assert_eq!(client_end_round(Proto::Extended, &c), Some(12));
    assert_eq!(
        client_end_round(Proto::Compact { t_commit_rounds: 8 }, &c),
        Some(30)
    );
    assert_eq!(client_end_round(recovery(true), &c), Some(30));
    assert_eq!(client_end_round(recovery(false), &c), Some(24));
}

#[test]
fn paper_latency_starts_at_injection_not_at_the_first_amplification() {
    let c = CommandReport {
        injected_round: Some(2),
        delivered_round: Some(11),
        ..command()
    };
    assert_eq!(
        paper_latency_sample(Proto::Compact { t_commit_rounds: 8 }, &c),
        Some((2, Some(25)))
    );
    let never = CommandReport {
        injected_round: None,
        delivered_round: None,
        ..command()
    };
    assert_eq!(paper_latency_sample(Proto::Extended, &never), None);
}
