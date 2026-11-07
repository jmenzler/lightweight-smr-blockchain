//! `SIM_PAR_STEP=on` runs stage (e)'s node applies on rayon; its reports must equal the serial ones.
//! The switch is read once per process, so each setting runs in a child copy of this binary.

use sim::smr::{
    ClientModel, PrefixMismatch, Proto, SmrScenario, TrafficPhase, point_mass_pmf, run_smr,
};
use sim::{BlockSchedule, Config};
use std::process::Command;

fn scenario(proto: Proto, n: usize) -> SmrScenario {
    SmrScenario {
        n,
        seed: 31,
        cfg: Config::default(),
        sigma: 1.0,
        proto,
        injections: vec![],
        max_rounds: 120,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(3),
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

/// Child entry point: prints one serialized report per engine that applies in `apply_all`.
#[test]
fn dump_reports() {
    if std::env::var("PAR_STEP_DUMP").is_err() {
        return;
    }
    let recovery = Proto::Recovery {
        t_window_rounds: 30,
        resend_until_acked: true,
        prefix_mismatch: PrefixMismatch::SkipBoundary,
    };
    for s in [
        scenario(
            Proto::Compact {
                t_commit_rounds: 20,
            },
            64,
        ),
        scenario(recovery, 48),
    ] {
        println!(
            "REPORT:{}",
            serde_json::to_string(&run_smr(&s)).expect("report")
        );
    }
}

fn reports_with(par_step: &str) -> Vec<String> {
    let out = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "dump_reports", "--nocapture", "--test-threads=1"])
        .env("PAR_STEP_DUMP", "1")
        .env("SIM_PAR_STEP", par_step)
        .output()
        .expect("spawn child");
    assert!(out.status.success(), "child failed: {out:?}");
    String::from_utf8(out.stdout)
        .expect("utf8")
        .lines()
        .filter_map(|l| l.split_once("REPORT:").map(|(_, report)| report.to_owned()))
        .collect()
}

#[test]
fn parallel_apply_reports_equal_the_serial_reports() {
    let (parallel, serial) = (reports_with("on"), reports_with("off"));
    assert_eq!(parallel.len(), 2);
    assert!(parallel == serial, "SIM_PAR_STEP=on changed a report");
}
