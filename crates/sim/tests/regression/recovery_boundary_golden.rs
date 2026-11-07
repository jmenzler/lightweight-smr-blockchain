//! Byte-identity tripwire for the recovery engine's checkpoint-boundary pass: every runner's
//! full serialized report, plus a per-round fold of each server's checkpoint, log and reset state,
//! over runs that cross many window ends with forks, rollbacks, adoptions and DEV-3 skips.
//! A boundary cache keyed on anything but allocation identity moves a value here. Regenerate
//! only as a deliberate, commit-noted act (`BOUNDARY_GOLDEN_PRINT=1` prints the table).

use protocol::recovery::RState;
use sim::smr::{
    ClientModel, ManualBlock, PrefixMismatch, Proto, SmrReport, SmrScenario, SmrState, SmrSummary,
    TrafficPhase, point_mass_pmf, run_smr_flat_observed, run_smr_grid_observed,
    run_smr_lean_observed, run_smr_observed,
};
use sim::{BlockSchedule, Config};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn fold(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn base(n: usize, seed: u64, t_window: u64, prefix_mismatch: PrefixMismatch) -> SmrScenario {
    SmrScenario {
        n,
        seed,
        cfg: Config::new(6, 3).expect("valid config"),
        sigma: 1.0,
        proto: Proto::Recovery {
            t_window_rounds: t_window,
            resend_until_acked: false,
            prefix_mismatch,
        },
        injections: Vec::new(),
        max_rounds: 120,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(4),
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

/// n = 32, T = 15, every server blocked in rounds 37-38: dated commit-path forks.
fn surge(seed: u64, prefix_mismatch: PrefixMismatch) -> SmrScenario {
    let mut s = base(32, seed, 15, prefix_mismatch);
    s.max_rounds = 140;
    s.manual_blocks = (0..32)
        .map(|node| ManualBlock {
            node,
            from_round: 37,
            to_round: Some(38),
        })
        .collect();
    s
}

fn cases() -> Vec<(&'static str, SmrScenario)> {
    let mut cascade = base(64, 424242, 10, PrefixMismatch::Abort);
    cascade.max_rounds = 70;
    cascade.traffic = Some(vec![TrafficPhase {
        from_round: 1,
        arrivals_pmf: point_mass_pmf(2),
    }]);
    let mut targets = vec![0.0; 31];
    for t in targets.iter_mut().skip(21) {
        *t = 0.6;
    }
    cascade.schedule = BlockSchedule::PerRoundSticky {
        background: 0.1,
        targets,
    };

    let mut skip_wide = base(32, 4330140, 26, PrefixMismatch::SkipBoundary);
    skip_wide.max_rounds = 208;
    skip_wide.schedule = BlockSchedule::FreshPerRound { fraction: 0.0 };

    let mut certs = base(48, 7, 8, PrefixMismatch::SkipBoundary);
    certs.certs = true;
    certs.max_rounds = 96;
    certs.schedule = BlockSchedule::FreshPerRound { fraction: 0.15 };
    certs.traffic = Some(vec![TrafficPhase {
        from_round: 1,
        arrivals_pmf: point_mass_pmf(3),
    }]);

    let mut wide = base(150, 11, 12, PrefixMismatch::SkipBoundary);
    wide.proto = Proto::Recovery {
        t_window_rounds: 12,
        resend_until_acked: true,
        prefix_mismatch: PrefixMismatch::SkipBoundary,
    };
    wide.schedule = BlockSchedule::FreshPerRound { fraction: 0.2 };
    wide.traffic = Some(vec![TrafficPhase {
        from_round: 1,
        arrivals_pmf: point_mass_pmf(6),
    }]);

    vec![
        ("surge_abort_0", surge(0, PrefixMismatch::Abort)),
        ("surge_abort_3", surge(3, PrefixMismatch::Abort)),
        ("surge_skip_0", surge(0, PrefixMismatch::SkipBoundary)),
        ("surge_skip_7", surge(7, PrefixMismatch::SkipBoundary)),
        ("cascade", cascade),
        ("skip_wide", skip_wide),
        ("certs", certs),
        ("wide", wide),
        ("wedge", dev3_cell(185, 6, 72, 0.0, "skip_boundary")),
        ("stale_spell", dev3_cell(226, 6, 60, 0.0, "skip_boundary")),
        ("oracle_abort", dev3_cell(4420003, 13, 130, 0.1, "abort")),
    ]
}

/// n = 32, four arrivals a round, repeated commits skipped: the DEV-3 skip / wedge cells.
fn dev3_cell(seed: u64, t: u64, max_rounds: usize, beta: f64, mismatch: &str) -> SmrScenario {
    let spec: sim::spec::SmrScenarioSpec = serde_json::from_value(serde_json::json!({
        "n": 32, "seed": seed, "k": 6, "ell": 3, "sigma": 1.0,
        "proto": {"kind": "recovery", "t_window_rounds": t, "resend_until_acked": false,
                  "prefix_mismatch": mismatch},
        "repeated_commit": "skip",
        "injections": [], "max_rounds": max_rounds,
        "traffic": {"kind": "pmf", "arrivals_pmf": [0.0, 0.0, 0.0, 0.0, 1.0]},
        "schedule": {"kind": "fresh_per_round", "fraction": beta},
    }))
    .expect("spec parses");
    SmrScenario::try_from(spec).expect("valid scenario")
}

#[derive(Default)]
struct Coverage {
    forked_boundaries: usize,
    rollbacks: u64,
    adoptions: u64,
    stale: u64,
    stamp_only_classes: u64,
    dated_commit_forks: usize,
    lineage_forks: usize,
    failed_runs: usize,
}

/// Per round: every server's (W, |P| or ⊥, |S|, R, |L| or ⊥, stale); counts mid-window adoptions.
struct Trace {
    hash: u64,
    windows: Vec<u64>,
    adoptions: u64,
    stale: u64,
    stamp_only_classes: u64,
}

impl Trace {
    fn observe(&mut self, st: &SmrState, n: usize, t_window: u64) {
        let round = st.round() as u64;
        self.windows.resize(n, 0);
        for i in 0..n {
            let nd = st.recovery_node(i).expect("recovery");
            let cp = nd.checkpoint();
            if cp.w > self.windows[i] && !round.is_multiple_of(t_window) {
                self.adoptions += 1;
            }
            self.windows[i] = cp.w;
            self.stale += u64::from(nd.stale_checkpoint());
            let fields = [
                cp.w,
                cp.p.as_ref().map_or(u64::MAX, |p| p.len() as u64),
                cp.s.logical_len(),
                nd.shared_state().logical_len(),
                r_code(nd.reset_state()),
                nd.log_seq().map_or(u64::MAX, |l| l.len() as u64),
                u64::from(nd.stale_checkpoint()),
            ];
            for f in fields {
                self.hash = fold(self.hash, &f.to_le_bytes());
            }
        }
        self.stamp_only_classes += u64::from(stamp_only_class(st, n));
    }
}

fn r_code(r: RState) -> u64 {
    match r {
        RState::NoReset => 0,
        RState::Reset => 1,
        RState::Bot => 2,
    }
}

/// Whether two newest-window checkpoints agree on S and on P as commands but not on P's stamps,
/// the one case the interning key separates beyond the fork signature.
fn stamp_only_class(st: &SmrState, n: usize) -> bool {
    let cps: Vec<_> = (0..n)
        .map(|i| st.recovery_node(i).expect("recovery").checkpoint())
        .collect();
    let max_w = cps.iter().map(|cp| cp.w).max().unwrap_or(0);
    let newest = cps.iter().filter(|cp| cp.w == max_w);
    newest.clone().any(|a| {
        newest.clone().any(|b| {
            a.s.logical_len() == b.s.logical_len()
                && a.s.sn_iter().eq(b.s.sn_iter())
                && a.p != b.p
                && a.p
                    .as_ref()
                    .map(|p| p.iter().map(|t| &t.entry).collect::<Vec<_>>())
                    == b.p
                        .as_ref()
                        .map(|p| p.iter().map(|t| &t.entry).collect::<Vec<_>>())
        })
    })
}

type Runner = fn(&SmrScenario, &mut dyn FnMut(&SmrState)) -> SmrReport;

const RUNNERS: [(&str, Runner); 3] = [
    ("full", run_smr_observed),
    ("lean", run_smr_lean_observed),
    ("grid", run_smr_grid_observed),
];

fn t_window(s: &SmrScenario) -> u64 {
    match s.proto {
        Proto::Recovery {
            t_window_rounds, ..
        } => t_window_rounds,
        _ => unreachable!("recovery cases only"),
    }
}

fn record(report: &SmrReport, trace: &Trace, cov: &mut Coverage) {
    let rec = report.recovery.as_ref().expect("recovery block");
    cov.forked_boundaries += rec.rounds.iter().filter(|r| r.cp_fork_k > 1).count();
    cov.rollbacks += rec
        .rounds
        .iter()
        .map(|r| u64::from(r.rollbacks))
        .sum::<u64>();
    cov.adoptions += trace.adoptions;
    cov.stale += trace.stale;
    cov.stamp_only_classes += trace.stamp_only_classes;
    cov.dated_commit_forks += usize::from(rec.commit_fork_round.is_some());
    cov.lineage_forks += usize::from(!rec.fork_ok);
    cov.failed_runs += usize::from(report.failure.is_some());
}

/// The spill's canonical stream; its header names the commit and is dropped. The file goes too.
fn spill_body(path: &str) -> String {
    let text = std::fs::read_to_string(path).expect("spill readable");
    std::fs::remove_file(path).expect("spill removable");
    let (header, body) = text.split_once('\n').expect("spill has a header line");
    assert!(header.contains("\"kind\":\"header\""), "{header}");
    body.to_string()
}

#[rustfmt::skip]
const GOLDEN: &[(&str, &str, u64, Option<u64>, u64)] = &[
    ("surge_abort_0", "full", 0xe4f04c586ae1e783, None, 0xc299bf1a01619981),
    ("surge_abort_0", "lean", 0x53ef2640c5f7ec1b, Some(0xc679796318e879fb), 0xc299bf1a01619981),
    ("surge_abort_0", "grid", 0x1aeeb7ec83bba9d9, Some(0xc679796318e879fb), 0xc299bf1a01619981),
    ("surge_abort_3", "full", 0x444c6b1efc4ae5fe, None, 0x97557bfa8386fd56),
    ("surge_abort_3", "lean", 0x90d865d15be70d48, Some(0x6149ecb7dc62d86b), 0x97557bfa8386fd56),
    ("surge_abort_3", "grid", 0x3198eab42e42037e, Some(0x6149ecb7dc62d86b), 0x97557bfa8386fd56),
    ("surge_skip_0", "full", 0xe4f04c586ae1e783, None, 0xc299bf1a01619981),
    ("surge_skip_0", "lean", 0x53ef2640c5f7ec1b, Some(0xc679796318e879fb), 0xc299bf1a01619981),
    ("surge_skip_0", "grid", 0x1aeeb7ec83bba9d9, Some(0xc679796318e879fb), 0xc299bf1a01619981),
    ("surge_skip_7", "full", 0x0846ad61f96077df, None, 0xc83ce511c645caac),
    ("surge_skip_7", "lean", 0x8dfc6281082777b6, Some(0xe6117d4107a5d663), 0xc83ce511c645caac),
    ("surge_skip_7", "grid", 0x30084eeacef99dd9, Some(0xe6117d4107a5d663), 0xc83ce511c645caac),
    ("cascade", "full", 0x921388e572edc1d8, None, 0xc7d33d8c4253e61a),
    ("cascade", "lean", 0x551e42a3b33810b1, Some(0xda77cf7abf8a9bbc), 0xc7d33d8c4253e61a),
    ("cascade", "grid", 0x742764dba68d6363, Some(0xda77cf7abf8a9bbc), 0xc7d33d8c4253e61a),
    ("skip_wide", "full", 0x51e087d9b26cbb89, None, 0xed37fcc51956d768),
    ("skip_wide", "lean", 0xcfefdfc1b167f6cb, Some(0x50da63cc22122b6a), 0xed37fcc51956d768),
    ("skip_wide", "grid", 0x7c790e34054e436d, Some(0x50da63cc22122b6a), 0xed37fcc51956d768),
    ("certs", "full", 0xfdb66723061f425b, None, 0x9dc2f054e889a63b),
    ("certs", "lean", 0x0e7cbae65ca17890, Some(0xe1e435b2a01ce748), 0x9dc2f054e889a63b),
    ("certs", "grid", 0x9939119a434864ad, Some(0xe1e435b2a01ce748), 0x9dc2f054e889a63b),
    ("wide", "full", 0x8640ea37b27e62c4, None, 0x1007853344663782),
    ("wide", "lean", 0x8640ea37b27e62c4, Some(0xcd157d7e8ccb45c8), 0x1007853344663782),
    ("wide", "grid", 0x558bc918ae197d14, Some(0xcd157d7e8ccb45c8), 0x1007853344663782),
    ("wedge", "full", 0xb2cc498b143fd67b, None, 0x2d3ba9a95fd8ea2c),
    ("wedge", "lean", 0xe6cad630b0d8835c, Some(0xe0d31626a3d1455a), 0x2d3ba9a95fd8ea2c),
    ("wedge", "grid", 0xa80dc59e842dcbcf, Some(0xe0d31626a3d1455a), 0x2d3ba9a95fd8ea2c),
    ("stale_spell", "full", 0x7d0f5adacc542527, None, 0xcbb3d64d5db685b8),
    ("stale_spell", "lean", 0xe7b5603852f73ac9, Some(0x0f5afca07d245efa), 0xcbb3d64d5db685b8),
    ("stale_spell", "grid", 0x900d2b5efc56b1a0, Some(0x0f5afca07d245efa), 0xcbb3d64d5db685b8),
    ("oracle_abort", "full", 0x32cde5a5855ff8d4, None, 0x95fbb06c23005bbc),
    ("oracle_abort", "lean", 0x662c48eeaa4ebbd3, Some(0x3bcba02bb6b9a418), 0x95fbb06c23005bbc),
    ("oracle_abort", "grid", 0x63c20d21145a68a6, Some(0x3bcba02bb6b9a418), 0x95fbb06c23005bbc),
];

#[test]
fn recovery_boundary_outputs_are_byte_identical() {
    let mut cov = Coverage::default();
    let mut actual = Vec::new();
    for (name, scenario) in cases() {
        let t = t_window(&scenario);
        for (runner_name, runner) in RUNNERS {
            let mut trace = Trace {
                hash: FNV_OFFSET,
                windows: Vec::new(),
                adoptions: 0,
                stale: 0,
                stamp_only_classes: 0,
            };
            let mut report = runner(&scenario, &mut |st| trace.observe(st, scenario.n, t));
            record(&report, &trace, &mut cov);
            let spill = report
                .spill_path
                .take()
                .map(|path| fold(FNV_OFFSET, spill_body(&path).as_bytes()));
            let json = serde_json::to_string(&report).expect("report serializes");
            actual.push((
                name,
                runner_name,
                fold(FNV_OFFSET, json.as_bytes()),
                spill,
                trace.hash,
            ));
        }
    }
    if std::env::var("BOUNDARY_GOLDEN_PRINT").is_ok() {
        for (name, runner, report, spill, trace) in &actual {
            let spill = spill.map_or("None".to_string(), |h| format!("Some({h:#018x})"));
            println!("    ({name:?}, {runner:?}, {report:#018x}, {spill}, {trace:#018x}),");
        }
        println!(
            "forked boundaries {}, rollbacks {}, adoptions {}, stale {}, stamp-only classes {}, \
             dated commit forks {}, lineage forks {}, failed runs {}",
            cov.forked_boundaries,
            cov.rollbacks,
            cov.adoptions,
            cov.stale,
            cov.stamp_only_classes,
            cov.dated_commit_forks,
            cov.lineage_forks,
            cov.failed_runs
        );
    }
    assert!(
        cov.forked_boundaries > 0,
        "precondition: a boundary with cp_fork_k > 1"
    );
    assert!(cov.rollbacks > 0, "precondition: a rollback");
    assert!(
        cov.adoptions > 0,
        "precondition: a mid-window checkpoint adoption"
    );
    assert!(cov.stale > 0, "precondition: a DEV-3 stale checkpoint");
    assert!(
        cov.stamp_only_classes > 0,
        "precondition: newest-window checkpoints differing only in P's stamps"
    );
    assert!(
        cov.dated_commit_forks > 0,
        "precondition: a dated commit-path fork"
    );
    assert!(
        cov.lineage_forks > 0,
        "precondition: a lineage fork verdict"
    );
    assert!(cov.failed_runs > 0, "precondition: an Abort run that stops");
    let spill_opted_out = ["SIM_SPILL", "SIM_RUNLOG"]
        .iter()
        .any(|v| std::env::var(v).as_deref() == Ok("off"));
    let expected: Vec<_> = GOLDEN
        .iter()
        .map(|&(name, runner, report, spill, trace)| {
            (
                name,
                runner,
                report,
                spill.filter(|_| !spill_opted_out),
                trace,
            )
        })
        .collect();
    assert_eq!(actual, expected, "recovery boundary outputs moved");
}

fn new_trace() -> Trace {
    Trace {
        hash: FNV_OFFSET,
        windows: Vec::new(),
        adoptions: 0,
        stale: 0,
        stamp_only_classes: 0,
    }
}

/// The flat runner over the same forks, rollbacks, adoptions and skips: identical node state
/// every round, identical spill, and a summary equal to the one read off the grid report.
#[test]
fn the_flat_runner_matches_the_grid_report_on_boundary_cases() {
    for (name, scenario) in cases() {
        let t = t_window(&scenario);
        let mut grid_trace = new_trace();
        let mut report =
            run_smr_grid_observed(&scenario, &mut |st| grid_trace.observe(st, scenario.n, t));
        let mut flat_trace = new_trace();
        let mut summary =
            run_smr_flat_observed(&scenario, &mut |st| flat_trace.observe(st, scenario.n, t));
        assert_eq!(flat_trace.hash, grid_trace.hash, "{name}: node state");
        let spill =
            |path: Option<String>| path.map(|p| fold(FNV_OFFSET, spill_body(&p).as_bytes()));
        assert_eq!(
            spill(summary.spill_path.take()),
            spill(report.spill_path.take()),
            "{name}: spill"
        );
        assert_eq!(
            summary,
            SmrSummary::from_report(&report, &scenario),
            "{name}: summary"
        );
    }
}
