//! The single-value simulator: Algorithm 1 rounds over n nodes under blocking schedules and partitions.

use protocol::MedianNode;

pub(crate) mod blocking;
pub use blocking::mask_schedule;
use blocking::{
    SnapshotDelay, adjust_sticky_mask, ceil_fraction_count, check_scenario_fit, checked_mask,
    compile_blocking, fraction_count, fraction_in_range, sample_blocked, sticky_with_background,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;

pub type Value = protocol::Value;
pub use protocol::Config;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Scenario {
    pub n: usize,
    pub seed: u64,
    pub cfg: Config,
    pub init: Init,
    pub max_rounds: usize,
    pub schedule: BlockSchedule,
    /// Component id per node: replies cross only within a component. This is link-level
    /// isolation, an extension beyond Def 1.1, which blocks whole servers. Partitioned nodes are
    /// neither blocked nor counted as non-useful in `RoundMetrics`, and a partitioned run never
    /// stops early on unanimity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<Vec<u32>>,
}

impl Scenario {
    /// The rules a spec enforces on parse, for scenarios built directly in Rust.
    pub fn validate(&self) -> Result<(), String> {
        if self.n < 1 {
            return Err("n must be at least 1".into());
        }
        self.cfg.check().map_err(|e| e.to_string())?;
        check_partition(self.partition.as_deref(), self.n)?;
        self.init.validate()?;
        self.schedule.validate(self.n)
    }
}

impl Init {
    fn validate(&self) -> Result<(), String> {
        match self {
            Init::Split { fraction } => fraction_in_range("split fraction", *fraction),
            Init::UniformRandom { k } if *k < 1 => {
                Err("uniform_random k must be at least 1".into())
            }
            Init::EvenSplit { values } if *values < 1 => {
                Err("even_split values must be at least 1".into())
            }
            Init::Weighted { weights, range } => {
                crate::smr::validate_weights("weighted init", "weights", weights)?;
                if *range < weights.len() as u64 {
                    return Err(format!(
                        "weighted range {range} smaller than its {} buckets",
                        weights.len()
                    ));
                }
                Ok(())
            }
            Init::WithUndecided {
                useful_fraction,
                inner,
            } => {
                if matches!(**inner, Init::WithUndecided { .. }) {
                    return Err("nested with_undecided is not supported".into());
                }
                fraction_in_range("useful_fraction", *useful_fraction)?;
                inner.validate()
            }
            Init::UniformRandom { .. } | Init::EvenSplit { .. } | Init::Distinct => Ok(()),
        }
    }
}

// Target sampling stays unrestricted; cross-component targets just never answer (RNG stream unchanged).
pub(crate) fn same_component(partition: Option<&[u32]>, i: usize, t: usize) -> bool {
    partition.is_none_or(|c| c[i] == c[t])
}

pub(crate) fn check_partition(partition: Option<&[u32]>, n: usize) -> Result<(), String> {
    match partition {
        Some(c) if c.len() != n => {
            Err(format!("partition names {} nodes, expected n={n}", c.len()))
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdaptivePolicy {
    BlockHolders,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum BlockSchedule {
    FreshPerRound {
        fraction: f64,
    },
    Permanent {
        fraction: f64,
    },
    /// Overlapping windows block the union of their nodes.
    Windows(Vec<BlockWindow>),
    PerRoundFractions(Vec<f64>),
    /// 0-late adaptive blocking: B_t reads the holders at the beginning of round t, one round later
    /// than Def 1.1 allows. A stress adversary outside the paper's model; the 1-late adversary is
    /// `AdaptiveAlphaLate { alpha: 1 }`.
    Adaptive0Late {
        fraction: f64,
        policy: AdaptivePolicy,
    },
    /// Definition 1.1 α-late adaptive blocking (α ≥ 1); Alg-1 only.
    AdaptiveAlphaLate {
        fraction: f64,
        alpha: u32,
        policy: AdaptivePolicy,
    },
    /// A sticky set adjusted to each round's target fraction, plus a fresh `background` sample
    /// blocked in the same round. The blocked set is their union, so a round blocks between
    /// max(target, background) and target + background of the nodes; `RoundMetrics::blocked`
    /// holds the realised count, which exceeds the declared target fraction when the two differ.
    PerRoundSticky {
        background: f64,
        targets: Vec<f64>,
    },
    /// Exploratory, not registered thesis evidence: the worst-case 1-late β-blocking adversary
    /// for availability (thesis/supervisor/optimal-adversary, proof v5). Every round blocks
    /// ⌊fraction·n⌋ servers drawn uniformly from those it did not block the round before.
    /// Needs fraction ≤ 1/2 (else too few fresh servers). "Previous round" means its own last
    /// mask: SMR `manual_blocks` are unioned on afterwards and are invisible to it.
    /// Worst case for availability only: it speeds up two-value agreement versus blocking nobody
    /// (optimal-adversary README), so it is no agreement stress test.
    Disjoint {
        fraction: f64,
    },
    /// Exploratory, not registered thesis evidence: the worst-case 1-late β-blocking adversary
    /// against two-value agreement (thesis/supervisor/optimal-adversary). Plays the exact DP's
    /// optimal action for the state (|B_{t−1}|, value counts of U_{t−1}) from a per-(n, ⌊βn⌋)
    /// table in `crates/sim/data/noise_controller/`; round 1 acts on the DP's start state
    /// (all n servers, split ⌊n/2⌋ : ⌈n/2⌉). Refused unless the scenario is the DP's model:
    /// the (6,3) rule, no partition, init `UniformRandom { k: 2 }` (iid, so U_1 is binomial).
    /// Alg-1 only; panics without a table for (n, ⌊βn⌋) or when U_{t−1} holds more than two values.
    NoiseController {
        fraction: f64,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BlockWindow {
    /// 1-based, inclusive.
    pub start_round: usize,
    pub rounds: usize,
    pub target: BlockTarget,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum BlockTarget {
    Nodes(Vec<u32>),
    SampleFraction(f64),
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum Init {
    Split {
        fraction: f64,
    },
    UniformRandom {
        k: Value,
    },
    Distinct,
    EvenSplit {
        values: usize,
    },
    Weighted {
        weights: Vec<f64>,
        range: u64,
    },
    /// Lemma 2.1/2.2 start: the first ⌈u₀·n⌉ nodes take `inner`'s value, the rest start ⊥.
    WithUndecided {
        useful_fraction: f64,
        inner: Box<Init>,
    },
}

/// What holds at the end of the run. Def 1.2 agreement and availability are reported separately (p. 6);
/// the paper leaves how to classify them open, so the convention is fixed here.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Outcome {
    /// All value holders hold one value from round `rounds` on. Unanimity cannot be undone, and the
    /// all-⊥ state counts as agreement on ⊥ (p. 19), so extinction after unanimity stays an agreement.
    /// `rounds` is the first round after which the holders are unanimous. Def 1.2 tests the useful
    /// servers of a round r, those holding a value at its start and not blocked in it; the first
    /// such r with unanimity is `rounds` or `rounds + 1`, never earlier.
    Agreement {
        value: Value,
        rounds: usize,
        availability: Availability,
    },
    /// Every node is ⊥ at round `rounds` without a single-value unanimity beforehand.
    AllUndecided { rounds: usize },
    /// The horizon was reached with several values still held.
    NoConvergence { rounds: usize },
}

/// Whether the system is still live when the run ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Availability {
    /// At least a third of the nodes hold a value after the last round.
    Held,
    /// Fewer than a third hold a value after the last round (`at`): the regime where the median
    /// rule trivially drives everyone to ⊥ (p. 19).
    Collapsing { at: usize },
    /// Every node is ⊥ since round `at`.
    Extinct { at: usize },
}

impl Outcome {
    /// Agreement with availability intact at the end of the run.
    pub fn survived(&self) -> bool {
        matches!(
            self,
            Outcome::Agreement {
                availability: Availability::Held,
                ..
            }
        )
    }

    /// Round at which every node became ⊥, if the run ended in extinction.
    pub fn extinction_round(&self) -> Option<usize> {
        match self {
            Outcome::AllUndecided { rounds }
            | Outcome::Agreement {
                availability: Availability::Extinct { at: rounds },
                ..
            } => Some(*rounds),
            _ => None,
        }
    }

    /// Row label and round for run tables; only "agreement" counts as survival.
    pub fn label(&self) -> (&'static str, usize) {
        match self {
            Outcome::Agreement {
                availability: Availability::Held,
                rounds,
                ..
            } => ("agreement", *rounds),
            Outcome::Agreement {
                availability: Availability::Collapsing { at },
                ..
            } => ("collapsing", *at),
            Outcome::Agreement {
                availability: Availability::Extinct { at },
                ..
            }
            | Outcome::AllUndecided { rounds: at } => ("died", *at),
            Outcome::NoConvergence { rounds } => ("censored", *rounds),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Trace {
    pub n: usize,
    pub seed: u64,
    pub initial: Vec<Option<Value>>,
    pub rounds: Vec<RoundTrace>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct RoundTrace {
    pub states: Vec<Option<Value>>,
    pub targets: Vec<Vec<u32>>,
    pub blocked: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RoundMetrics {
    pub holders: u32,
    pub undecided: u32,
    pub blocked: u32,
    pub useful: u32,
    pub distinct_values: u32,
}

#[derive(Debug, serde::Serialize)]
pub struct Report {
    pub outcome: Outcome,
    pub metrics: Vec<RoundMetrics>,
}

pub fn run(scenario: &Scenario) -> Outcome {
    run_inner(scenario, false).0
}

pub fn run_report(scenario: &Scenario) -> Report {
    let (outcome, metrics, _) = run_inner(scenario, false);
    Report { outcome, metrics }
}

pub fn run_traced(scenario: &Scenario) -> (Outcome, Trace) {
    let (outcome, _, trace) = run_inner(scenario, true);
    (outcome, trace.expect("recording was requested"))
}

pub fn run_traced_report(scenario: &Scenario) -> (Report, Trace) {
    let (outcome, metrics, trace) = run_inner(scenario, true);
    (
        Report { outcome, metrics },
        trace.expect("recording was requested"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundStatus {
    pub metrics: RoundMetrics,
    pub agreed: Option<Value>,
    pub all_hold: bool,
    pub all_undecided: bool,
}

/// Incremental stepping core; its RNG stream is byte-identical to the batch runner's.
pub struct SimState {
    n: usize,
    cfg: Config,
    nodes: Vec<MedianNode>,
    rng: ChaCha12Rng,
    round: usize,
    trace: Option<Trace>,
    metrics: Vec<RoundMetrics>,
    agreed_since: Option<(Value, usize)>,
    partition: Option<Vec<u32>>,
}

impl SimState {
    pub fn new(
        n: usize,
        cfg: Config,
        init: &Init,
        seed: u64,
        record_trace: bool,
        partition: Option<&[u32]>,
    ) -> SimState {
        check_partition(partition, n).expect("invalid partition");
        // Single seeded RNG is the only entropy source; replay-from-seed depends on it.
        let mut rng = ChaCha12Rng::seed_from_u64(seed);
        let nodes: Vec<MedianNode> = (0..n)
            .map(|i| match initial_state(init, i, n, &mut rng) {
                Some(v) => MedianNode::new(v, cfg),
                None => MedianNode::new_undecided(cfg),
            })
            .collect();
        let trace = record_trace.then(|| Trace {
            n,
            seed,
            initial: nodes.iter().map(MedianNode::answer).collect(),
            rounds: Vec::new(),
        });
        SimState {
            n,
            cfg,
            nodes,
            rng,
            round: 0,
            trace,
            metrics: Vec::new(),
            agreed_since: None,
            partition: partition.map(<[u32]>::to_vec),
        }
    }

    pub fn sample_mask(&mut self, count: usize) -> Vec<bool> {
        sample_blocked(&mut self.rng, self.n, count)
    }

    pub fn step_sampled(&mut self, count: usize) -> RoundStatus {
        let blocked = sample_blocked(&mut self.rng, self.n, count);
        self.step_with(&blocked)
    }

    pub fn step_fraction(&mut self, fraction: f64) -> RoundStatus {
        self.step_sampled(self.blocked_count(fraction))
    }

    pub fn blocked_count(&self, fraction: f64) -> usize {
        fraction_count(fraction, self.n)
    }

    pub fn step_masked(&mut self, blocked: &[bool]) -> RoundStatus {
        assert_eq!(blocked.len(), self.n, "a mask names every node");
        self.step_with(blocked)
    }

    pub fn adjust_sticky(&mut self, mask: &mut [bool], count: usize) {
        adjust_sticky_mask(&mut self.rng, mask, count, self.n);
    }

    /// The batch driver's sticky round mask: adjust `sticky` to `count`, then union a background sample.
    pub fn sticky_round_mask(
        &mut self,
        sticky: &mut [bool],
        count: usize,
        background_count: usize,
    ) -> Vec<bool> {
        sticky_with_background(&mut self.rng, sticky, count, background_count, self.n)
    }

    pub(crate) fn rng_mut(&mut self) -> &mut ChaCha12Rng {
        &mut self.rng
    }

    pub(crate) fn holder_ids(&self) -> Vec<usize> {
        (0..self.n)
            .filter(|&i| self.nodes[i].answer().is_some())
            .collect()
    }

    /// Value counts of this round's useful set: current holders not in `blocked`, ascending by value.
    pub(crate) fn useful_value_counts(&self, blocked: &[bool]) -> Vec<(Value, usize)> {
        let mut counts = std::collections::BTreeMap::new();
        for (node, _) in self.nodes.iter().zip(blocked).filter(|(_, b)| !**b) {
            if let Some(v) = node.answer() {
                *counts.entry(v).or_insert(0) += 1;
            }
        }
        counts.into_iter().collect()
    }

    pub fn round(&self) -> usize {
        self.round
    }

    pub fn metrics(&self) -> &[RoundMetrics] {
        &self.metrics
    }

    pub fn trace(&self) -> Option<&Trace> {
        self.trace.as_ref()
    }

    pub fn agreed_since(&self) -> Option<(Value, usize)> {
        self.agreed_since
    }

    /// Whether `status` can never change again, given that no further round blocks anyone.
    /// Unanimity with every node holding a value is stable only when every reply can arrive,
    /// which a partition prevents: cross-component targets never answer.
    pub fn is_absorbed(&self, status: &RoundStatus, no_more_blocking: bool) -> bool {
        status.agreed.is_some() && status.all_hold && no_more_blocking && self.partition.is_none()
    }

    pub fn into_parts(self) -> (Vec<RoundMetrics>, Option<Trace>) {
        (self.metrics, self.trace)
    }

    fn step_with(&mut self, blocked: &[bool]) -> RoundStatus {
        let n = self.n;
        let cfg = self.cfg;
        self.round += 1;
        let snapshot: Vec<Option<Value>> = self.nodes.iter().map(MedianNode::answer).collect();

        let partition = self.partition.as_deref();
        let mut targets_log: Vec<Vec<u32>> = Vec::new();
        let mut inboxes: Vec<Vec<Value>> = Vec::with_capacity(n);
        for i in 0..n {
            let mut replies = Vec::with_capacity(cfg.k);
            let mut targets = Vec::with_capacity(cfg.k);
            for _ in 0..cfg.k {
                let t = self.rng.random_range(0..n);
                targets.push(t as u32);
                if !blocked[i]
                    && !blocked[t]
                    && same_component(partition, i, t)
                    && let Some(v) = snapshot[t]
                {
                    replies.push(v);
                }
            }
            targets_log.push(targets);
            inboxes.push(replies);
        }

        for (node, inbox) in self.nodes.iter_mut().zip(&inboxes) {
            node.step(inbox, &mut self.rng);
        }

        if let Some(t) = self.trace.as_mut() {
            t.rounds.push(RoundTrace {
                states: self.nodes.iter().map(MedianNode::answer).collect(),
                targets: targets_log,
                blocked: (0..n).filter(|&i| blocked[i]).map(|i| i as u32).collect(),
            });
        }

        let holders = self
            .nodes
            .iter()
            .filter(|node| node.answer().is_some())
            .count() as u32;
        let round_metrics = RoundMetrics {
            holders,
            undecided: n as u32 - holders,
            blocked: blocked.iter().filter(|&&b| b).count() as u32,
            useful: snapshot
                .iter()
                .zip(blocked)
                .filter(|(s, b)| s.is_some() && !**b)
                .count() as u32,
            distinct_values: self
                .nodes
                .iter()
                .filter_map(MedianNode::answer)
                .collect::<std::collections::HashSet<_>>()
                .len() as u32,
        };
        self.metrics.push(round_metrics);

        let (agreed, all_undecided) = match population_state(&self.nodes) {
            Population::Agreed(value) => {
                if self.agreed_since.is_none() {
                    self.agreed_since = Some((value, self.round));
                }
                (Some(value), false)
            }
            Population::AllUndecided => (None, true),
            Population::Mixed => (None, false),
        };
        RoundStatus {
            metrics: round_metrics,
            agreed,
            all_hold: self.nodes.iter().all(|node| node.answer().is_some()),
            all_undecided,
        }
    }
}

pub(crate) fn run_inner(
    scenario: &Scenario,
    record_trace: bool,
) -> (Outcome, Vec<RoundMetrics>, Option<Trace>) {
    scenario
        .validate()
        .unwrap_or_else(|e| panic!("invalid scenario: {e}"));
    let n = scenario.n;
    let mut state = SimState::new(
        n,
        scenario.cfg,
        &scenario.init,
        scenario.seed,
        record_trace,
        scenario.partition.as_deref(),
    );

    check_scenario_fit(
        &scenario.schedule,
        scenario.cfg,
        &scenario.init,
        scenario.partition.as_deref(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    // Blocking compiles before round 1; declaration order is part of the RNG stream.
    let mut blocking = compile_blocking(&scenario.schedule, n, state.rng_mut());
    let mut delay = SnapshotDelay::new(blocking.knowledge());

    for round in 1..=scenario.max_rounds {
        let mask = checked_mask(blocking.as_mut(), &delay.view(round), state.rng_mut(), n);
        delay.record_useful(|| state.useful_value_counts(&mask));
        let status = state.step_masked(&mask);
        delay.record(|| state.holder_ids());

        if status.all_undecided {
            let outcome = match state.agreed_since() {
                Some((value, rounds)) => Outcome::Agreement {
                    value,
                    rounds,
                    availability: Availability::Extinct { at: round },
                },
                None => Outcome::AllUndecided { rounds: round },
            };
            let (metrics, trace) = state.into_parts();
            return (outcome, metrics, trace);
        }
        // Absorbing only if all hold and nobody can be blocked later: with ⊥ nodes, holders can still go extinct.
        if state.is_absorbed(&status, blocking.done_blocking_after(round)) {
            let (value, rounds) = state.agreed_since().expect("agreed implies recorded");
            let (metrics, trace) = state.into_parts();
            let outcome = Outcome::Agreement {
                value,
                rounds,
                availability: Availability::Held,
            };
            return (outcome, metrics, trace);
        }
    }
    // The median rule never invents values, so a single surviving value is final.
    let outcome = match state.agreed_since() {
        Some((value, rounds)) => Outcome::Agreement {
            value,
            rounds,
            availability: horizon_availability(state.metrics(), n, scenario.max_rounds),
        },
        None => Outcome::NoConvergence {
            rounds: scenario.max_rounds,
        },
    };
    let (metrics, trace) = state.into_parts();
    (outcome, metrics, trace)
}

fn horizon_availability(metrics: &[RoundMetrics], n: usize, horizon: usize) -> Availability {
    let holders = metrics
        .last()
        .expect("an agreement implies a round ran")
        .holders as usize;
    if holders * 3 < n {
        Availability::Collapsing { at: horizon }
    } else {
        Availability::Held
    }
}

/// One weighted bucket draw (exactly one f64); the SMR traffic count-draw shares this shape.
pub fn draw_weighted_index(rng: &mut ChaCha12Rng, weights: &[f64]) -> usize {
    let total: f64 = weights.iter().sum();
    let mut r = rng.random::<f64>() * total;
    let mut pick = weights.iter().rposition(|w| *w > 0.0).unwrap_or(0);
    for (v, w) in weights.iter().enumerate() {
        if r < *w {
            pick = v;
            break;
        }
        r -= w;
    }
    pick
}

/// One node's initial value; call for i in 0..n over one RNG, since the draw order is the assignment.
pub fn initial_state(init: &Init, i: usize, n: usize, rng: &mut ChaCha12Rng) -> Option<Value> {
    match init {
        Init::Split { fraction } => Some(if i < ceil_fraction_count(*fraction, n) {
            0
        } else {
            1
        }),
        Init::UniformRandom { k } => Some(rng.random_range(0..*k)),
        Init::Distinct => Some(i as Value),
        Init::EvenSplit { values } => Some((i as u128 * *values as u128 / n as u128) as Value),
        Init::Weighted { weights, range } => {
            let pick = draw_weighted_index(rng, weights);
            let m = weights.len() as u128;
            let lo = (*range as u128 * pick as u128 / m) as u64;
            let hi = (*range as u128 * (pick as u128 + 1) / m) as u64;
            // 1-value slices draw nothing, keeping dense sampling at one draw per node.
            if hi - lo > 1 {
                Some(lo + rng.random_range(0..hi - lo))
            } else {
                Some(lo)
            }
        }
        // ⊥ nodes draw nothing, so the stream equals the inner init at the smaller useful count.
        Init::WithUndecided {
            useful_fraction,
            inner,
        } => {
            let useful = ceil_fraction_count(*useful_fraction, n);
            (i < useful)
                .then(|| initial_state(inner, i, useful.max(1), rng))
                .flatten()
        }
    }
}

enum Population {
    Agreed(Value),
    AllUndecided,
    Mixed,
}

fn population_state(nodes: &[MedianNode]) -> Population {
    let mut agreed: Option<Value> = None;
    let mut any_value = false;
    for node in nodes {
        if let Some(v) = node.x_i() {
            any_value = true;
            match agreed {
                None => agreed = Some(v),
                Some(a) if a != v => return Population::Mixed,
                _ => {}
            }
        }
    }
    if !any_value {
        return Population::AllUndecided;
    }
    // All value-holding servers agree after this round; ⊥ nodes allowed (see `Outcome::Agreement` for how this relates to Def 1.2).
    Population::Agreed(agreed.expect("any_value implies Some"))
}
