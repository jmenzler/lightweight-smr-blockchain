//! RNG-contract tripwire for the partition reply gate. An unpartitioned run
//! consumes a fixed stream: the gate conditions an already-drawn target and
//! never draws by itself. These digests cover all four round loops and change
//! only when their draw contract deliberately changes.

use sim::smr::{
    ClientModel, Injection, Proto, SmrReport, SmrScenario, TrafficPhase, point_mass_pmf, run_smr,
};
use sim::{BlockSchedule, Config, Init, Scenario, run_traced};

use crate::digest_fields;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn fold(mut h: u64, s: &str) -> u64 {
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// FNV-1a over the whole serialized trace — every round's states, sampled
/// targets and blocked set, so any shift in the draw stream shows up.
fn alg1_digest(scenario: &Scenario) -> (usize, u64) {
    let (_, trace) = run_traced(scenario);
    let json = serde_json::to_string(&trace).expect("trace serializes");
    (trace.rounds.len(), fold(FNV_OFFSET, &json))
}

/// Shape counts beside the hash so a collapsed-to-nothing regression cannot
/// hide behind a digest.
fn smr_digest(report: &SmrReport) -> (usize, usize, u64) {
    // Observer-derived landmarks are not part of the draw stream.
    let commands: Vec<_> = report
        .commands
        .iter()
        .cloned()
        .map(|c| sim::smr::CommandReport {
            committed_round: None,
            injected_round: None,
            index_agreed_round: None,
            ..c
        })
        .collect();
    let json = serde_json::to_string(&commands).expect("commands serialize");
    let metrics = digest_fields::without_derived_fields(
        &serde_json::to_string(&report.metrics).expect("metrics serialize"),
    );
    (
        report.commands.len(),
        report.metrics.len(),
        fold(fold(FNV_OFFSET, &json), &metrics),
    )
}

fn smr_base(proto: Proto, n: usize, seed: u64, rate: usize) -> SmrScenario {
    SmrScenario {
        n,
        seed,
        cfg: Config::default(),
        sigma: 5.0,
        proto,
        injections: vec![Injection {
            round: 2,
            client: 1,
            op: 7,
            target: None,
        }],
        max_rounds: 60,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.05 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(rate),
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
fn alg1_round_loop_stream_is_unchanged() {
    let scenario = Scenario {
        n: 256,
        seed: 424_242,
        cfg: Config::default(),
        init: Init::Split { fraction: 0.5 },
        max_rounds: 120,
        schedule: BlockSchedule::Permanent { fraction: 0.2 },
        partition: None,
    };
    assert_eq!(alg1_digest(&scenario), (120, 1_302_229_882_311_473_182));
}

#[test]
fn extended_round_loop_stream_is_unchanged() {
    let report = run_smr(&smr_base(Proto::Extended, 64, 424_243, 2));
    assert_eq!(smr_digest(&report), (121, 60, 8_466_897_486_141_275_370));
}

#[test]
fn compact_round_loop_stream_is_unchanged() {
    let report = run_smr(&smr_base(
        Proto::Compact {
            t_commit_rounds: 21,
        },
        64,
        424_244,
        2,
    ));
    assert_eq!(smr_digest(&report), (121, 60, 15_074_880_660_899_109_434));
}

#[test]
fn recovery_round_loop_stream_is_unchanged() {
    let report = run_smr(&smr_base(
        Proto::Recovery {
            t_window_rounds: 20,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        64,
        424_245,
        2,
    ));
    assert_eq!(smr_digest(&report), (121, 60, 1_004_709_871_762_392_784));
}
