//! Validated wire format for scenarios; a spec never bypasses the construction checks.

use crate::{
    AdaptivePolicy, BlockSchedule, BlockTarget, BlockWindow, Config, Init, Scenario, Value,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioSpec {
    pub n: usize,
    pub seed: u64,
    pub k: usize,
    pub ell: usize,
    pub init: InitSpec,
    pub max_rounds: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<ScheduleSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InitSpec {
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<u64>,
    },
    WithUndecided {
        useful_fraction: f64,
        inner: Box<InitSpec>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduleSpec {
    FreshPerRound {
        fraction: f64,
    },
    Permanent {
        fraction: f64,
    },
    Windows {
        windows: Vec<WindowSpec>,
    },
    PerRoundFractions {
        fractions: Vec<f64>,
    },
    PerRoundSticky {
        fractions: Vec<f64>,
        #[serde(default)]
        background: f64,
    },
    #[serde(rename = "adaptive_0late")]
    Adaptive0Late {
        fraction: f64,
        policy: AdaptivePolicy,
    },
    AdaptiveAlphaLate {
        fraction: f64,
        alpha: u32,
        policy: AdaptivePolicy,
    },
    /// Exploratory worst-case 1-late availability adversary; fraction ≤ 1/2.
    Disjoint {
        fraction: f64,
    },
    /// Exploratory worst-case 1-late adversary against two-value agreement; needs a table for (n, ⌊βn⌋).
    NoiseController {
        fraction: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowSpec {
    pub start_round: usize,
    pub rounds: usize,
    pub target: TargetSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetSpec {
    Nodes { ids: Vec<u32> },
    SampleFraction { fraction: f64 },
}

impl TryFrom<ScenarioSpec> for Scenario {
    type Error = String;

    fn try_from(spec: ScenarioSpec) -> Result<Scenario, String> {
        let cfg = Config::new(spec.k, spec.ell).map_err(|e| e.to_string())?;
        if spec.n < 1 {
            return Err("n must be at least 1".into());
        }
        crate::check_partition(spec.partition.as_deref(), spec.n)?;
        let scenario = Scenario {
            n: spec.n,
            seed: spec.seed,
            cfg,
            init: convert_init(&spec.init)?,
            max_rounds: spec.max_rounds,
            schedule: match &spec.schedule {
                None => BlockSchedule::FreshPerRound { fraction: 0.0 },
                Some(schedule) => convert_schedule(schedule, spec.n)?,
            },
            partition: spec.partition,
        };
        crate::blocking::check_scenario_fit(
            &scenario.schedule,
            scenario.cfg,
            &scenario.init,
            scenario.partition.as_deref(),
        )?;
        Ok(scenario)
    }
}

fn fraction_in_range(name: &str, fraction: f64) -> Result<f64, String> {
    crate::consensus::blocking::fraction_in_range(name, fraction).map(|()| fraction)
}

fn convert_init(spec: &InitSpec) -> Result<Init, String> {
    match spec {
        InitSpec::Split { fraction } => Ok(Init::Split {
            fraction: fraction_in_range("split fraction", *fraction)?,
        }),
        InitSpec::UniformRandom { k } => {
            if *k < 1 {
                return Err("uniform_random k must be at least 1".into());
            }
            Ok(Init::UniformRandom { k: *k })
        }
        InitSpec::Distinct => Ok(Init::Distinct),
        InitSpec::EvenSplit { values } => {
            if *values < 1 {
                return Err("even_split values must be at least 1".into());
            }
            Ok(Init::EvenSplit { values: *values })
        }
        InitSpec::Weighted { weights, range } => {
            crate::smr::validate_weights("weighted init", "weights", weights)?;
            let range = range.unwrap_or(weights.len() as u64);
            if range < weights.len() as u64 {
                return Err(format!(
                    "weighted range {range} smaller than its {} buckets",
                    weights.len()
                ));
            }
            Ok(Init::Weighted {
                weights: weights.clone(),
                range,
            })
        }
        InitSpec::WithUndecided {
            useful_fraction,
            inner,
        } => {
            if matches!(**inner, InitSpec::WithUndecided { .. }) {
                return Err("nested with_undecided is not supported".into());
            }
            Ok(Init::WithUndecided {
                useful_fraction: fraction_in_range("useful_fraction", *useful_fraction)?,
                inner: Box::new(convert_init(inner)?),
            })
        }
    }
}

fn convert_schedule(spec: &ScheduleSpec, n: usize) -> Result<BlockSchedule, String> {
    match spec {
        ScheduleSpec::FreshPerRound { fraction } => Ok(BlockSchedule::FreshPerRound {
            fraction: fraction_in_range("fresh_per_round fraction", *fraction)?,
        }),
        ScheduleSpec::Permanent { fraction } => Ok(BlockSchedule::Permanent {
            fraction: fraction_in_range("permanent fraction", *fraction)?,
        }),
        ScheduleSpec::Adaptive0Late { fraction, policy } => Ok(BlockSchedule::Adaptive0Late {
            fraction: fraction_in_range("adaptive_0late fraction", *fraction)?,
            policy: *policy,
        }),
        ScheduleSpec::AdaptiveAlphaLate {
            fraction,
            alpha,
            policy,
        } => {
            if *alpha == 0 {
                return Err(
                    "adaptive_alpha_late alpha must be >= 1 (alpha = 0 is the adaptive_0late kind)"
                        .to_string(),
                );
            }
            Ok(BlockSchedule::AdaptiveAlphaLate {
                fraction: fraction_in_range("adaptive_alpha_late fraction", *fraction)?,
                alpha: *alpha,
                policy: *policy,
            })
        }
        ScheduleSpec::PerRoundFractions { fractions } => {
            for fraction in fractions {
                fraction_in_range("per_round fraction", *fraction)?;
            }
            Ok(BlockSchedule::PerRoundFractions(fractions.clone()))
        }
        ScheduleSpec::PerRoundSticky {
            fractions,
            background,
        } => {
            for fraction in fractions {
                fraction_in_range("per_round sticky fraction", *fraction)?;
            }
            Ok(BlockSchedule::PerRoundSticky {
                background: fraction_in_range("sticky background fraction", *background)?,
                targets: fractions.clone(),
            })
        }
        ScheduleSpec::Disjoint { fraction } => {
            let fraction = fraction_in_range("disjoint fraction", *fraction)?;
            crate::blocking::disjoint_precondition(fraction)?;
            Ok(BlockSchedule::Disjoint { fraction })
        }
        ScheduleSpec::NoiseController { fraction } => {
            let schedule = BlockSchedule::NoiseController {
                fraction: fraction_in_range("noise_controller fraction", *fraction)?,
            };
            crate::blocking::check_schedule(&schedule, n)?;
            Ok(schedule)
        }
        ScheduleSpec::Windows { windows } => Ok(BlockSchedule::Windows(
            windows
                .iter()
                .map(|w| convert_window(w, n))
                .collect::<Result<_, _>>()?,
        )),
    }
}

fn convert_window(spec: &WindowSpec, n: usize) -> Result<BlockWindow, String> {
    if spec.start_round < 1 {
        return Err("window start_round must be at least 1".into());
    }
    if spec.rounds < 1 {
        return Err("window rounds must be at least 1".into());
    }
    let target = match &spec.target {
        TargetSpec::Nodes { ids } => {
            if let Some(bad) = ids.iter().find(|&&id| id as usize >= n) {
                return Err(format!("window node id {bad} out of range for n={n}"));
            }
            BlockTarget::Nodes(ids.clone())
        }
        TargetSpec::SampleFraction { fraction } => {
            BlockTarget::SampleFraction(fraction_in_range("window sample fraction", *fraction)?)
        }
    };
    Ok(BlockWindow {
        start_round: spec.start_round,
        rounds: spec.rounds,
        target,
    })
}

use crate::smr::{
    ClientModel, Injection, ManualBlock, MergePolicy, PrefixMismatch, Proto, RepeatedCommit,
    SmrScenario, TrafficPhase,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmrScenarioSpec {
    pub n: usize,
    pub seed: u64,
    pub k: usize,
    pub ell: usize,
    pub sigma: f64,
    pub proto: ProtoSpec,
    pub injections: Vec<InjectionSpec>,
    pub max_rounds: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<ScheduleSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic: Option<TrafficSpec>,
    #[serde(default, skip_serializing_if = "ClientModel::is_unique")]
    pub client_model: ClientModel,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub certs: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_blocks: Vec<ManualBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub halt_on_violation: bool,
    #[serde(default, skip_serializing_if = "MergePolicy::is_default")]
    pub merge_policy: MergePolicy,
    #[serde(
        default,
        skip_serializing_if = "crate::smr::is_execute",
        with = "crate::smr::RepeatedCommitWire"
    )]
    pub repeated_commit: RepeatedCommit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrafficSpec {
    Pmf { arrivals_pmf: Vec<f64> },
    Phases { phases: Vec<TrafficPhaseSpec> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficPhaseSpec {
    pub from_round: usize,
    pub arrivals_pmf: Vec<f64>,
}

fn convert_traffic(spec: &TrafficSpec) -> Vec<TrafficPhase> {
    match spec {
        TrafficSpec::Pmf { arrivals_pmf } => vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: arrivals_pmf.clone(),
        }],
        TrafficSpec::Phases { phases } => phases
            .iter()
            .map(|p| TrafficPhase {
                from_round: p.from_round,
                arrivals_pmf: p.arrivals_pmf.clone(),
            })
            .collect(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProtoSpec {
    Extended,
    Compact {
        t_commit_rounds: u64,
    },
    Recovery {
        t_window_rounds: u64,
        /// Required: true is the paper's client (resend until a contacted server acks, p. 25).
        resend_until_acked: bool,
        #[serde(
            default,
            skip_serializing_if = "crate::smr::is_abort",
            with = "crate::smr::PrefixMismatchWire"
        )]
        prefix_mismatch: PrefixMismatch,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectionSpec {
    pub round: usize,
    pub client: u32,
    pub op: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<u32>,
}

impl TryFrom<SmrScenarioSpec> for SmrScenario {
    type Error = String;

    fn try_from(spec: SmrScenarioSpec) -> Result<SmrScenario, String> {
        let cfg = Config::new(spec.k, spec.ell).map_err(|e| e.to_string())?;
        let scenario = SmrScenario {
            n: spec.n,
            seed: spec.seed,
            cfg,
            sigma: spec.sigma,
            proto: match spec.proto {
                ProtoSpec::Extended => Proto::Extended,
                ProtoSpec::Compact { t_commit_rounds } => Proto::Compact { t_commit_rounds },
                ProtoSpec::Recovery {
                    t_window_rounds,
                    resend_until_acked,
                    prefix_mismatch,
                } => Proto::Recovery {
                    t_window_rounds,
                    resend_until_acked,
                    prefix_mismatch,
                },
            },
            injections: spec
                .injections
                .iter()
                .map(|i| Injection {
                    round: i.round,
                    client: i.client,
                    op: i.op,
                    target: i.target,
                })
                .collect(),
            max_rounds: spec.max_rounds,
            schedule: match &spec.schedule {
                None => BlockSchedule::FreshPerRound { fraction: 0.0 },
                Some(schedule) => convert_schedule(schedule, spec.n)?,
            },
            traffic: spec.traffic.as_ref().map(convert_traffic),
            client_model: spec.client_model,
            certs: spec.certs,
            manual_blocks: spec.manual_blocks.clone(),
            partition: spec.partition.clone(),
            halt_on_violation: spec.halt_on_violation,
            merge_policy: spec.merge_policy,
            repeated_commit: spec.repeated_commit,
        };
        scenario.validate()?;
        Ok(scenario)
    }
}
