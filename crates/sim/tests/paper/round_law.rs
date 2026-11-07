//! One unblocked Alg-1 round has the law the analysis uses (Lemma 2.4, Cor 2.5): with a fraction
//! u of the nodes holding a value, every node, holder or not, receives at least ℓ replies with
//! probability P(Bin(k, u) ≥ ℓ), and the value it adopts is the median of ℓ independent draws from
//! the holders' values. Targets are uniform over all n nodes with replacement, the own node
//! included; n = 20 makes a self-excluding or class-dependent target law visible.

use sim::{Config, Init, SimState};

const N: usize = 20;
const SEEDS: u64 = 4000;

fn first_round_states(seed: u64) -> Vec<Option<u64>> {
    let init = Init::WithUndecided {
        useful_fraction: 0.5,
        inner: Box::new(Init::Split { fraction: 0.3 }),
    };
    let mut state = SimState::new(N, Config::default(), &init, seed, true, None);
    state.step_sampled(0);
    state.trace().expect("recorded").rounds[0].states.clone()
}

#[test]
fn success_rate_and_adopted_value_follow_the_analysis() {
    // Nodes 0..10 start with a value (3 zeros, 7 ones); nodes 10..20 start at ⊥.
    let (mut ok_holder, mut ok_bottom, mut zeros, mut adopted) = (0u64, 0u64, 0u64, 0u64);
    for seed in 0..SEEDS {
        for (i, state) in first_round_states(seed).iter().enumerate() {
            let Some(v) = state else { continue };
            if i < N / 2 {
                ok_holder += 1;
            } else {
                ok_bottom += 1;
            }
            adopted += 1;
            zeros += u64::from(*v == 0);
        }
    }
    let trials = SEEDS * (N as u64 / 2);
    let success = 42.0 / 64.0; // P(Bin(6, 1/2) ≥ 3)
    for (class, ok) in [("holders", ok_holder), ("bottom nodes", ok_bottom)] {
        let rate = ok as f64 / trials as f64;
        assert!(
            (rate - success).abs() < 0.01,
            "{class}: {rate} vs {success}"
        );
    }
    let p: f64 = 0.3;
    let median_of_three = 3.0 * p * p - 2.0 * p.powi(3);
    let zero_rate = zeros as f64 / adopted as f64;
    assert!(
        (zero_rate - median_of_three).abs() < 0.01,
        "P(adopt 0 | success) = {zero_rate} vs {median_of_three}"
    );
}
