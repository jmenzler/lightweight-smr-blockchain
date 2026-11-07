//! Run configuration: one JSON file shared byte-identical by every host.

use serde::{Deserialize, Serialize};

pub use sim::spec::{ScenarioSpec, SmrScenarioSpec};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum ScenarioKind {
    Median(ScenarioSpec),
    Smr(SmrScenarioSpec),
    Gossip(GossipSpec),
    Priority(ScenarioSpec),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GossipSpec {
    pub n: usize,
    pub seed: u64,
    pub k: usize,
    pub ell: usize,
    /// Servers 0..holders start with `x`; the rest with `x0`.
    pub holders: usize,
    pub x: u64,
    pub x0: u64,
    pub max_rounds: usize,
    #[serde(default)]
    pub schedule: Option<sim::spec::ScheduleSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    Barrier,
    Timer,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkSpec {
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub jitter_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRunSpec {
    pub scenario: ScenarioKind,
    pub sync: SyncMode,
    #[serde(default = "default_round_ms")]
    pub round_ms: u64,
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
    #[serde(default)]
    pub start_offset_ms: u64,
    /// Cap on one frame including payload padding. A reply that exceeds it is not sent and counts as
    /// `encode_err`; Alg 3 replies carry the whole log, so long or padded runs must raise it.
    #[serde(default = "default_max_frame_bytes")]
    pub max_frame_bytes: u32,
    /// Wire-level padding per command payload (cores stay u64).
    #[serde(default)]
    pub payload_bytes: u32,
    #[serde(default)]
    pub network: NetworkSpec,
    /// Ops whose position/prefix digests ride in every RoundRecord.
    #[serde(default)]
    pub tracked_ops: Vec<u64>,
}

fn default_round_ms() -> u64 {
    200
}
fn default_listen_port() -> u16 {
    9000
}
fn default_max_frame_bytes() -> u32 {
    64 * 1024 * 1024
}

impl NodeRunSpec {
    /// Parse + validate (reuses the sim family's TryFrom validation).
    pub fn from_json(json: &str) -> Result<NodeRunSpec, String> {
        let spec: NodeRunSpec = serde_json::from_str(json).map_err(|e| e.to_string())?;
        spec.validate()?;
        Ok(spec)
    }

    pub(crate) fn value_scenario(&self) -> Result<sim::Scenario, String> {
        match &self.scenario {
            ScenarioKind::Median(s) | ScenarioKind::Priority(s) => {
                sim::Scenario::try_from(s.clone())
            }
            ScenarioKind::Smr(_) => Err("SMR scenario has no value-mode form".to_string()),
            ScenarioKind::Gossip(g) => sim::Scenario::try_from(ScenarioSpec {
                n: g.n,
                seed: g.seed,
                k: g.k,
                ell: g.ell,
                init: sim::spec::InitSpec::Distinct,
                max_rounds: g.max_rounds,
                schedule: g.schedule.clone(),
                partition: None,
            }),
        }
    }

    /// Every server's initial value, drawn in node-id order from the run's own `"init"` stream (not
    /// the sim's shared stream, so random inits differ from the sim's at the same seed).
    pub fn initial_values(&self) -> Result<Vec<Option<u64>>, String> {
        if let ScenarioKind::Gossip(g) = &self.scenario {
            return Ok((0..g.n)
                .map(|i| Some(if i < g.holders { g.x } else { g.x0 }))
                .collect());
        }
        let scenario = self.value_scenario()?;
        let mut init_rng = crate::rng::derive_rng(scenario.seed, "init", 0);
        Ok((0..scenario.n)
            .map(|i| sim::initial_state(&scenario.init, i, scenario.n, &mut init_rng))
            .collect())
    }

    /// The value a gossip/priority run must spread: x, or the maximum initial value.
    pub fn target_value(&self) -> Result<Option<u64>, String> {
        match &self.scenario {
            ScenarioKind::Gossip(g) => Ok(Some(g.x)),
            ScenarioKind::Priority(_) => Ok(self.initial_values()?.into_iter().flatten().max()),
            ScenarioKind::Median(_) | ScenarioKind::Smr(_) => Ok(None),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Some(field) = self.unsupported_sim_field() {
            return Err(format!("{field} is not supported in networked mode"));
        }
        match &self.scenario {
            ScenarioKind::Median(_) | ScenarioKind::Gossip(_) | ScenarioKind::Priority(_) => {
                if let ScenarioKind::Gossip(g) = &self.scenario {
                    if g.holders == 0 || g.holders > g.n {
                        return Err(format!(
                            "gossip holders must be in 1..=n (got {} of {})",
                            g.holders, g.n
                        ));
                    }
                    if g.x == g.x0 {
                        return Err("gossip x must differ from the dummy x0".to_string());
                    }
                }
                if let ScenarioKind::Priority(p) = &self.scenario
                    && matches!(p.init, sim::spec::InitSpec::WithUndecided { .. })
                {
                    return Err(
                        "priority rule has no undecided start (Def 3.5 begins with values)"
                            .to_string(),
                    );
                }
                self.value_scenario()?;
            }
            ScenarioKind::Smr(s) => {
                sim::smr::SmrScenario::try_from(s.clone())?;
            }
        }
        if self.k() > usize::from(u16::MAX) {
            return Err(format!(
                "k = {} exceeds the u16 reply-slot width of the wire format",
                self.k()
            ));
        }
        if u32::try_from(self.n()).is_err() {
            return Err("n exceeds the u32 node-id width of the wire format".to_string());
        }
        if self.round_ms == 0 {
            return Err("round_ms must be positive (CP-4 timer rounds)".to_string());
        }
        if self.max_frame_bytes < 1024 {
            return Err("max_frame_bytes below 1KiB cannot carry any message".to_string());
        }
        if self.listen_port == 0 {
            return Err("listen_port 0 is not routable".to_string());
        }
        if self.payload_bytes > self.max_frame_bytes {
            return Err("payload_bytes exceeds the frame cap".to_string());
        }
        Ok(())
    }

    /// Sim-only knobs the node would otherwise silently ignore (or panic on).
    fn unsupported_sim_field(&self) -> Option<&'static str> {
        let (schedule, partition) = match &self.scenario {
            ScenarioKind::Median(s) | ScenarioKind::Priority(s) => {
                (&s.schedule, s.partition.is_some())
            }
            ScenarioKind::Gossip(g) => (&g.schedule, false),
            ScenarioKind::Smr(s) => {
                if matches!(s.proto, sim::spec::ProtoSpec::Recovery { .. }) {
                    return Some("recovery proto");
                }
                if !s.manual_blocks.is_empty() {
                    return Some("manual_blocks");
                }
                if !s.client_model.is_unique() {
                    return Some("pool client_model");
                }
                if s.certs {
                    return Some("certs");
                }
                if s.halt_on_violation {
                    return Some("halt_on_violation");
                }
                if !s.merge_policy.is_default() {
                    return Some("merge_policy");
                }
                (&s.schedule, s.partition.is_some())
            }
        };
        if partition {
            return Some("partition");
        }
        use sim::spec::ScheduleSpec as S;
        // Exhaustive so a new schedule must decide here: the node precomputes masks without node state.
        match schedule {
            None
            | Some(
                S::FreshPerRound { .. }
                | S::Permanent { .. }
                | S::Windows { .. }
                | S::PerRoundFractions { .. }
                | S::PerRoundSticky { .. }
                | S::Disjoint { .. },
            ) => None,
            Some(S::Adaptive0Late { .. }) => Some("adaptive_0late schedule"),
            Some(S::AdaptiveAlphaLate { .. }) => Some("adaptive_alpha_late schedule"),
            Some(S::NoiseController { .. }) => Some("noise_controller schedule"),
        }
    }

    pub fn n(&self) -> usize {
        match &self.scenario {
            ScenarioKind::Median(s) | ScenarioKind::Priority(s) => s.n,
            ScenarioKind::Smr(s) => s.n,
            ScenarioKind::Gossip(s) => s.n,
        }
    }

    fn k(&self) -> usize {
        match &self.scenario {
            ScenarioKind::Median(s) | ScenarioKind::Priority(s) => s.k,
            ScenarioKind::Smr(s) => s.k,
            ScenarioKind::Gossip(s) => s.k,
        }
    }

    pub fn max_rounds(&self) -> usize {
        match &self.scenario {
            ScenarioKind::Median(s) | ScenarioKind::Priority(s) => s.max_rounds,
            ScenarioKind::Smr(s) => s.max_rounds,
            ScenarioKind::Gossip(s) => s.max_rounds,
        }
    }
}
