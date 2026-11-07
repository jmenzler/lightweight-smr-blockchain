//! recovered_round follows Def 1.4: equal executed sequences among the useful servers, so one
//! permanently blocked server cannot make a benign run read as never recovered.

use sim::smr::{ClientModel, Injection, ManualBlock, PrefixMismatch, Proto, SmrScenario, run_smr};
use sim::{BlockSchedule, BlockTarget, BlockWindow, Config};

fn surge_scenario(n: usize, t: u64, seed: u64) -> SmrScenario {
    SmrScenario {
        n,
        seed,
        cfg: Config::new(6, 3).expect("valid config"),
        sigma: 1.0,
        proto: Proto::Recovery {
            t_window_rounds: t,
            resend_until_acked: false,
            prefix_mismatch: PrefixMismatch::Abort,
        },
        injections: vec![
            Injection {
                round: 5,
                client: 1,
                op: 7,
                target: None,
            },
            Injection {
                round: 200,
                client: 1,
                op: 8,
                target: None,
            },
        ],
        max_rounds: 300,
        schedule: BlockSchedule::Windows(vec![BlockWindow {
            start_round: 101,
            rounds: 20,
            target: BlockTarget::Nodes((0..n as u32).collect()),
        }]),
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

// Lemma 6.10, pp. 35-37: recovery within 3T of the surge end among useful servers.
#[test]
fn surge_recovery_is_reported_despite_one_permanently_blocked_server() {
    let (n, t) = (64usize, 20u64);
    let release = 120usize;
    let clean = surge_scenario(n, t, 3);
    let baseline = run_smr(&clean).recovered_round(n, t);
    assert!(baseline.is_some(), "an unblocked run recovers");

    let mut blocked = clean;
    blocked.manual_blocks = vec![ManualBlock {
        node: 0,
        from_round: 1,
        to_round: None,
    }];
    let report = run_smr(&blocked);
    assert!(report.safety_ok);
    let got = report.recovered_round(n, t);
    assert!(
        got.is_some_and(|r| r <= release + 3 * t as usize),
        "recovered_round {got:?} (baseline {baseline:?})"
    );
}
