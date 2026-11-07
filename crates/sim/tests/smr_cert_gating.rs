//! §5 certificate state is a configuration choice: Algorithm 6 AS PRINTED
//! stores none — the §6.1 box's preconditions are S, L, R and C = (S, P, W),
//! and the storage requirement is stated in §5 (pp. 27–28), not in the box —
//! while the extension layer stores the last two commands per client with their
//! Merkle chains. The paper names no certificate-free recovery variant, so this
//! is "Alg 6 without §5's extension", never "the paper's base Algorithm 6".
//!
//! The gate is therefore only defensible if it changes nothing a run reports
//! and fails loudly where a consumer needs what was never built — which is what
//! this file pins.

use sim::smr::{
    ClientModel, Proto, SmrScenario, SmrState, TrafficPhase, point_mass_pmf, run_smr, run_smr_lean,
};
use sim::spec::SmrScenarioSpec;
use sim::{BlockSchedule, Config};

/// Loaded recovery over several boundaries: enough traffic that the per-client
/// windows are populated rather than empty, so a cert-on run really does carry
/// the state a cert-off run drops.
fn cell(certs: bool) -> SmrScenario {
    SmrScenario {
        n: 24,
        seed: 771_400,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Recovery {
            t_window_rounds: 20,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        injections: vec![],
        max_rounds: 120,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(1),
        }]),
        client_model: ClientModel::Unique,
        certs,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

/// Structural inertness, made testable instead of asserted: cert maintenance
/// draws no RNG and `Checkpoint::eq` deliberately skips the payload, so the
/// gate must move no metric row, no command record and no landmark. This is
/// what licenses running the whole campaign catalog gated off.
#[test]
fn the_gate_moves_no_reported_field_in_either_mode() {
    for lean in [false, true] {
        let run = if lean { run_smr_lean } else { run_smr };
        let mut off = run(&cell(false));
        let mut on = run(&cell(true));
        // Provenance, not observation: each run names its own spill file.
        off.spill_path = None;
        on.spill_path = None;
        assert_eq!(
            off, on,
            "the §5 layer changed a reported field (lean = {lean})"
        );
    }
}

#[test]
fn base_algorithm_6_checkpoints_carry_no_certificate_state() {
    let mut state = SmrState::new(
        24,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: 4,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        1.0,
        771_401,
        &[],
        &[],
        ClientModel::Unique,
        None,
    );
    let no_block = vec![false; 24];
    for _ in 0..12 {
        state.step_masked(&no_block);
    }
    for i in 0..24 {
        assert!(
            state.checkpoint_certs(i).is_none(),
            "node {i} kept §5 state on a run that never mounted the layer"
        );
    }
}

#[test]
fn mounting_the_layer_puts_the_state_back() {
    let mut state = SmrState::new_with_certs(
        24,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: 4,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        1.0,
        771_401,
        &[],
        &[],
        ClientModel::Unique,
        None,
        true,
        Default::default(),
        Default::default(),
    );
    let no_block = vec![false; 24];
    for _ in 0..12 {
        state.step_masked(&no_block);
    }
    for i in 0..24 {
        assert!(
            state.checkpoint_certs(i).is_some(),
            "node {i} lost the §5 state the run asked for"
        );
    }
}

/// R7: a cert consumer reached by a gated-off run must name the flag, never
/// degrade to an empty forest — an empty forest verifies nothing and would
/// read as a refuted certificate rather than as a misconfigured run.
#[test]
#[should_panic(expected = "set certs: true in the scenario")]
fn a_cert_consumer_on_a_gated_off_run_panics_naming_the_flag() {
    let mut state = SmrState::new(
        8,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: 4,
            resend_until_acked: true,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        1.0,
        771_402,
        &[],
        &[],
        ClientModel::Unique,
        None,
    );
    let mut harness = sim::certs::CertHarness::new(8);
    state.step_masked(&[false; 8]);
    harness.observe_round_rec(&state);
}

/// R3: existing spec bytes and ledger rows are unchanged, which is what keeps
/// every recorded run comparable across this round.
#[test]
fn the_gate_is_absent_from_the_wire_while_it_is_off() {
    let bare = r#"{"n":8,"seed":1,"k":6,"ell":3,"sigma":1.0,
        "proto":{"kind":"recovery","t_window_rounds":4,"resend_until_acked":false},
        "injections":[],"max_rounds":8}"#;
    let spec: SmrScenarioSpec = serde_json::from_str(bare).expect("a spec without the gate parses");
    assert!(!spec.certs, "an absent gate is off");
    let scenario = SmrScenario::try_from(spec.clone()).expect("valid scenario");
    assert!(!scenario.certs);
    assert!(
        !serde_json::to_string(&spec).unwrap().contains("certs"),
        "an off gate must not reach the wire"
    );
    assert!(
        !serde_json::to_string(&scenario).unwrap().contains("certs"),
        "an off gate must not reach a ledger row"
    );

    let mounted: SmrScenarioSpec = serde_json::from_str(
        r#"{"n":8,"seed":1,"k":6,"ell":3,"sigma":1.0,
           "proto":{"kind":"recovery","t_window_rounds":4,"resend_until_acked":false},
           "injections":[],"max_rounds":8,"certs":true}"#,
    )
    .expect("a spec naming the gate parses");
    assert!(mounted.certs);
    assert!(
        SmrScenario::try_from(mounted.clone()).unwrap().certs,
        "the gate must reach the engine scenario"
    );
    let round_tripped: SmrScenarioSpec =
        serde_json::from_str(&serde_json::to_string(&mounted).unwrap()).unwrap();
    assert!(round_tripped.certs, "a mounted gate survives a round trip");
}
