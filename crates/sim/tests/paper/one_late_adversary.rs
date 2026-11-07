//! The 1-late adversary (Def 1.1, pp. 4-5, 14) versus the 0-late stress adversary.

use rand::SeedableRng;
use rand::seq::index::sample;
use rand_chacha::ChaCha12Rng;
use sim::spec::ScenarioSpec;
use sim::{Config, Init, Scenario, SimState, run_report, run_traced};

const ZERO_LATE: &str = r#""kind":"adaptive_0late""#;
const ONE_LATE: &str = r#""kind":"adaptive_alpha_late","alpha":1"#;
const OLD_ONE_LATE: &str = r#""kind":"adaptive_1late""#;

fn holders_of(states: &[Option<u64>]) -> Vec<usize> {
    (0..states.len()).filter(|&i| states[i].is_some()).collect()
}

fn spec_json(kind: &str, n: usize, seed: u64, fraction: f64, rounds: usize, init: &str) -> String {
    format!(
        r#"{{"n":{n},"seed":{seed},"k":6,"ell":3,"max_rounds":{rounds},"init":{init},"schedule":{{{kind},"fraction":{fraction},"policy":"block_holders"}}}}"#
    )
}

fn parse(json: &str) -> Result<Scenario, String> {
    let spec: ScenarioSpec = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Scenario::try_from(spec)
}

fn scenario(kind: &str, n: usize, seed: u64, fraction: f64, rounds: usize, init: &str) -> Scenario {
    parse(&spec_json(kind, n, seed, fraction, rounds, init)).expect("valid scenario")
}

// Independent 1-late BlockHolders adversary written from p. 14: B_t uses the state at the
// beginning of round t-1 only (round 1: nothing known; round 2: the initial state).
fn one_late_mask(
    rng: &mut ChaCha12Rng,
    known: Option<&[usize]>,
    n: usize,
    count: usize,
) -> Vec<bool> {
    let mut mask = vec![false; n];
    match known {
        Some(h) if h.len() > count => {
            for i in sample(rng, h.len(), count) {
                mask[h[i]] = true;
            }
        }
        Some(h) => {
            for &i in h {
                mask[i] = true;
            }
            let rest: Vec<usize> = (0..n).filter(|i| !mask[*i]).collect();
            for i in sample(rng, rest.len(), (count - h.len()).min(rest.len())) {
                mask[rest[i]] = true;
            }
        }
        None => {
            for i in sample(rng, n, count.min(n)) {
                mask[i] = true;
            }
        }
    }
    mask
}

struct RefRun {
    died: bool,
    states_begin: Vec<Vec<Option<u64>>>,
    blocked: Vec<Vec<bool>>,
}

fn reference_one_late_run(n: usize, seed: u64, beta: f64, max_rounds: usize) -> RefRun {
    let init = Init::Split { fraction: 0.5 };
    let mut st = SimState::new(n, Config::new(6, 3).unwrap(), &init, seed, true, None);
    let count = (beta * n as f64).floor() as usize;
    let mut adv = ChaCha12Rng::seed_from_u64(seed ^ 0x1A7E_1A7E);
    let mut states_begin: Vec<Vec<Option<u64>>> =
        vec![st.trace().expect("trace on").initial.clone()];
    let mut blocked = Vec::new();
    for r in 1..=max_rounds {
        let known = (r >= 2).then(|| holders_of(&states_begin[r - 2]));
        let mask = one_late_mask(&mut adv, known.as_deref(), n, count);
        let status = st.step_masked(&mask);
        states_begin.push(st.trace().unwrap().rounds.last().unwrap().states.clone());
        blocked.push(mask);
        if status.all_undecided {
            return RefRun {
                died: true,
                states_begin,
                blocked,
            };
        }
    }
    RefRun {
        died: false,
        states_begin,
        blocked,
    }
}

// Trace row i holds the end of round i+1; the beginning of round r is row r-2 (initial for r = 1).
fn begin_states(trace: &sim::Trace, r: usize) -> &[Option<u64>] {
    if r == 1 {
        &trace.initial
    } else {
        &trace.rounds[r - 2].states
    }
}

#[test]
fn the_old_one_late_wire_name_is_rejected_loudly() {
    let json = spec_json(
        OLD_ONE_LATE,
        100,
        0,
        0.1,
        10,
        r#"{"kind":"split","fraction":0.5}"#,
    );
    let err = parse(&json).expect_err("the 0-late schedule must not keep its 1-late wire name");
    assert!(
        err.contains("adaptive_1late") && err.contains("adaptive_0late"),
        "the error names the old kind and the renamed one: {err}"
    );
}

#[test]
fn the_zero_late_schedule_is_a_distinct_kind_from_alpha_late() {
    let init = r#"{"kind":"split","fraction":0.5}"#;
    let zero = scenario(ZERO_LATE, 100, 0, 0.1, 10, init);
    let one = scenario(ONE_LATE, 100, 0, 0.1, 10, init);
    assert_ne!(zero.schedule, one.schedule);
    assert!(format!("{:?}", zero.schedule).contains("Adaptive0Late"));
}

// M.27/M.34, pp. 4, 14: a 1-late adversary picks B_r from the state at the beginning of round r-1.
#[test]
fn alpha_late_one_blocks_only_holders_known_one_round_late() {
    let init = r#"{"kind":"with_undecided","useful_fraction":0.5,"inner":{"kind":"split","fraction":0.5}}"#;
    let (_, trace) = run_traced(&scenario(ONE_LATE, 40, 7, 0.1, 6, init));
    let budget = 4;
    // Round 2 falls back to a uniform draw, so the check starts at round 3.
    for r in 3..=trace.rounds.len() {
        let known = holders_of(begin_states(&trace, r - 1));
        if known.len() < budget {
            continue;
        }
        let outside: Vec<u32> = trace.rounds[r - 1]
            .blocked
            .iter()
            .copied()
            .filter(|id| !known.contains(&(*id as usize)))
            .collect();
        assert!(
            outside.is_empty(),
            "round {r}: blocked {outside:?} were not holders at the beginning of round {}",
            r - 1
        );
    }
}

// The zero-late schedule reads the state at the beginning of the current round.
#[test]
fn zero_late_blocks_holders_of_the_current_round() {
    let init = r#"{"kind":"with_undecided","useful_fraction":0.5,"inner":{"kind":"split","fraction":0.5}}"#;
    let (_, trace) = run_traced(&scenario(ZERO_LATE, 40, 7, 0.1, 6, init));
    let mut checked = 0;
    for r in 2..=trace.rounds.len() {
        let current = holders_of(begin_states(&trace, r));
        if current.len() < 4 {
            continue;
        }
        assert!(
            trace.rounds[r - 1]
                .blocked
                .iter()
                .all(|id| current.contains(&(*id as usize))),
            "round {r}: a 0-late adversary blocks only current holders"
        );
        checked += 1;
    }
    assert!(checked >= 2, "need rounds that exercise the policy");
}

// A1.ASSUME.13, p. 12: B_{t+1} is independent of round-t randomness, so it cannot tell
// round-t losers from round-t keepers among the servers it saw holding at the beginning of t.
#[test]
fn alpha_late_one_mask_is_blind_to_previous_round_outcomes() {
    fn rates(states: &[Vec<Option<u64>>], blocked: &[Vec<bool>], n: usize) -> (f64, f64, usize) {
        let (mut lost, mut lost_hit, mut kept, mut kept_hit) = (0usize, 0usize, 0usize, 0usize);
        for r in 3..=blocked.len() {
            let t = r - 1;
            for i in 0..n {
                let held_begin_t = states[t - 1][i].is_some();
                let free_in_t = !blocked[t - 1][i];
                if !(held_begin_t && free_in_t) {
                    continue;
                }
                let holds_begin_r = states[r - 1][i].is_some();
                let hit = blocked[r - 1][i];
                if holds_begin_r {
                    kept += 1;
                    kept_hit += usize::from(hit);
                } else {
                    lost += 1;
                    lost_hit += usize::from(hit);
                }
            }
        }
        (
            lost_hit as f64 / lost as f64,
            kept_hit as f64 / kept as f64,
            lost,
        )
    }

    let (n, beta, rounds) = (1000, 0.1, 40);
    for seed in 0..3u64 {
        let rr = reference_one_late_run(n, seed, beta, rounds);
        let (lost_r, kept_r, _) = rates(&rr.states_begin, &rr.blocked, n);
        assert!(lost_r > 0.5 * kept_r, "reference sanity, seed {seed}");

        let init = r#"{"kind":"split","fraction":0.5}"#;
        let (_, trace) = run_traced(&scenario(ONE_LATE, n, seed, beta, rounds, init));
        let states: Vec<Vec<Option<u64>>> = std::iter::once(trace.initial.clone())
            .chain(trace.rounds.iter().map(|row| row.states.clone()))
            .collect();
        let blocked: Vec<Vec<bool>> = trace
            .rounds
            .iter()
            .map(|row| {
                let mut m = vec![false; n];
                for &id in &row.blocked {
                    m[id as usize] = true;
                }
                m
            })
            .collect();
        let (lost, kept, lost_count) = rates(&states, &blocked, n);
        assert!(
            lost_count > 100,
            "seed {seed}: too few losers ({lost_count})"
        );
        assert!(
            lost > 0.5 * kept,
            "alpha-late 1 seed {seed}: keeper rate {kept:.4} vs loser rate {lost:.4}; B_t depends on R_(t-1)"
        );
    }
}

// M.ASSUME.1, pp. 5, 14: the 1-late kind survives like the independent 1-late adversary, while
// the 0-late schedule at the same budget kills the population.
#[test]
fn only_the_alpha_late_one_kind_survives_like_a_one_late_adversary_at_beta_013() {
    let (n, beta, rounds, seeds) = (1000, 0.13, 300, 40u64);
    let init = r#"{"kind":"split","fraction":0.5}"#;
    let (mut one_late, mut zero_late, mut reference) = (0, 0, 0);
    for seed in 0..seeds {
        let died = |kind| {
            let report = run_report(&scenario(kind, n, seed, beta, rounds, init));
            report.outcome.extinction_round().is_some()
        };
        one_late += u32::from(died(ONE_LATE));
        zero_late += u32::from(died(ZERO_LATE));
        reference += u32::from(reference_one_late_run(n, seed, beta, rounds).died);
    }
    assert!(
        one_late <= reference + 4,
        "alpha-late 1 killed {one_late}/{seeds}, the independent 1-late adversary {reference}/{seeds}"
    );
    assert!(
        zero_late >= 3 * seeds as u32 / 4,
        "the 0-late schedule killed only {zero_late}/{seeds} at beta {beta}"
    );
}
