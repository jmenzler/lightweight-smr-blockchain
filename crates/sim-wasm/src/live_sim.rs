use crate::helpers::{check_fraction, error_json, recorded_schedule};
use sim::spec::ScenarioSpec;
use sim::{RoundStatus, Scenario, SimState, Trace, run_traced_report};
use wasm_bindgen::prelude::wasm_bindgen;

const TARGETS_MAX_N: usize = 2000;

/// Run a full scenario from spec JSON; returns `{trace, report}` or `{error}` JSON.
#[wasm_bindgen]
pub fn run_scenario_json(spec_json: &str) -> String {
    match run_scenario(spec_json) {
        Ok(json) => json,
        Err(error) => serde_json::json!({ "error": error }).to_string(),
    }
}

fn run_scenario(spec_json: &str) -> Result<String, String> {
    let spec: ScenarioSpec = serde_json::from_str(spec_json).map_err(|e| e.to_string())?;
    let scenario = Scenario::try_from(spec)?;
    let (report, mut trace) = run_traced_report(&scenario);
    strip_targets_when_large(&mut trace);
    Ok(serde_json::json!({ "trace": trace, "report": report }).to_string())
}

fn strip_targets_when_large(trace: &mut Trace) {
    if trace.n > TARGETS_MAX_N {
        for round in &mut trace.rounds {
            round.targets.clear();
        }
    }
}

/// Stepwise simulator session whose recorded history replays byte-identical via `run_scenario_json`.
/// The blocking fraction is chosen interactively each round, so a session is not a 1-late adversary run.
#[wasm_bindgen]
pub struct LiveSim {
    state: SimState,
    base: ScenarioSpec,
    fractions: Vec<f64>,
    last: Option<RoundStatus>,
    sticky: bool,
    background: f64,
    mask: Vec<bool>,
}

#[wasm_bindgen]
impl LiveSim {
    /// Spec's `schedule` is ignored: no up-front mask draw may touch the RNG stream.
    #[wasm_bindgen(constructor)]
    pub fn new(spec_json: &str) -> Result<LiveSim, String> {
        LiveSim::new_with_mode(spec_json, false)
    }

    /// `sticky`: the blocked set persists between steps, adjusted by delta-sampling.
    pub fn new_with_mode(spec_json: &str, sticky: bool) -> Result<LiveSim, String> {
        let spec: ScenarioSpec = serde_json::from_str(spec_json).map_err(|e| e.to_string())?;
        let scenario = Scenario::try_from(spec.clone())?;
        let background = match (sticky, &scenario.schedule) {
            (true, sim::BlockSchedule::FreshPerRound { fraction }) => *fraction,
            _ => 0.0,
        };
        let state = SimState::new(
            scenario.n,
            scenario.cfg,
            &scenario.init,
            scenario.seed,
            true,
            scenario.partition.as_deref(),
        );
        let mask = vec![false; scenario.n];
        Ok(LiveSim {
            state,
            base: spec,
            fractions: Vec::new(),
            last: None,
            sticky,
            background,
            mask,
        })
    }

    /// Advance one round at `fraction` blocking (an error JSON outside [0, 1]); no-op once absorbed so the replay length matches.
    pub fn step(&mut self, fraction: f64) -> String {
        if let Err(error) = check_fraction(fraction) {
            return error_json(error);
        }
        let count = self.state.blocked_count(fraction);
        let background_count = self.state.blocked_count(self.background);
        if let Some(last) = self.last
            && (last.all_undecided
                || self
                    .state
                    .is_absorbed(&last, count == 0 && background_count == 0))
        {
            return self.status_json(&last, true);
        }
        let status = if self.sticky {
            let mask = self
                .state
                .sticky_round_mask(&mut self.mask, count, background_count);
            self.state.step_masked(&mask)
        } else {
            self.state.step_sampled(count)
        };
        self.fractions.push(fraction);
        self.last = Some(status);
        self.status_json(&status, false)
    }

    pub fn export_scenario(&self) -> String {
        let mut spec = self.base.clone();
        spec.schedule = Some(recorded_schedule(
            self.sticky,
            &self.fractions,
            self.background,
        ));
        spec.max_rounds = self.fractions.len();
        serde_json::to_string(&spec).expect("spec serializes")
    }

    pub fn trace_json(&self) -> String {
        let trace = self.state.trace().expect("live sessions always record");
        let mut value = serde_json::to_value(trace).expect("trace serializes");
        if trace.n > TARGETS_MAX_N {
            for round in value["rounds"].as_array_mut().expect("rounds array") {
                round["targets"] = serde_json::json!([]);
            }
        }
        value.to_string()
    }

    fn status_json(&self, status: &RoundStatus, absorbed: bool) -> String {
        let trace = self.state.trace().expect("live sessions always record");
        let round = trace.rounds.last().expect("status implies a stepped round");
        let targets: &[Vec<u32>] = if trace.n > TARGETS_MAX_N {
            &[]
        } else {
            &round.targets
        };
        serde_json::json!({
            "round": self.state.round(),
            "states": round.states,
            "targets": targets,
            "blocked": round.blocked,
            "metrics": status.metrics,
            "agreed_value": status.agreed,
            "all_hold": status.all_hold,
            "all_undecided": status.all_undecided,
            "absorbed": absorbed,
        })
        .to_string()
    }
}
