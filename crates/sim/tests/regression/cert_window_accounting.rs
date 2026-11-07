//! Reported certificate-window bytes are a function of the logical chain content, not of Vec
//! capacity, which depends on clone and growth history.

use protocol::certificates::StoredCommand;
use sim::smr::{
    ClientModel, PrefixMismatch, Proto, SmrScenario, SmrState, TrafficPhase, point_mass_pmf,
    run_smr_observed,
};
use sim::{BlockSchedule, Config};

#[test]
fn accounted_cert_window_bytes_equal_the_logical_chain_size() {
    let s = SmrScenario {
        n: 32,
        seed: 975_300,
        cfg: Config::new(6, 3).expect("valid config"),
        sigma: 1.0,
        proto: Proto::Recovery {
            t_window_rounds: 20,
            resend_until_acked: false,
            prefix_mismatch: PrefixMismatch::Abort,
        },
        injections: Vec::new(),
        max_rounds: 120,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(1),
        }]),
        client_model: ClientModel::Unique,
        certs: true,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    };
    let mut last = None;
    let report = run_smr_observed(&s, &mut |st: &SmrState| {
        let acc = st.memory_accounting().expect("recovery accounting");
        let mut seen = std::collections::HashSet::new();
        let mut expected = 0u64;
        for i in 0..s.n {
            let cp = st.recovery_node(i).expect("node").checkpoint_shared();
            if !seen.insert(std::sync::Arc::as_ptr(&cp) as usize) {
                continue;
            }
            let Some(certs) = cp.certs.as_ref() else {
                continue;
            };
            let mut held = 0usize;
            for j in 0..2_000u32 {
                let client = sim::smr::AUTO_CLIENT_BASE + j;
                if let Some((prev, newest)) = certs.last_two(client) {
                    held += 1;
                    expected += (newest.chain.len() * 32) as u64;
                    if let Some(p) = prev {
                        expected += (p.chain.len() * 32) as u64;
                    }
                }
            }
            let entry = size_of::<u32>() + size_of::<(Option<StoredCommand>, StoredCommand)>();
            expected += ((held * entry) as u64 * 3) / 2;
        }
        let reported = acc
            .rows
            .get("checkpoint.certs.windows")
            .copied()
            .unwrap_or(0);
        last = Some((reported, expected));
    });
    assert!(report.safety_ok);
    let (reported, expected) = last.expect("observed");
    assert_eq!(reported, expected);
}
