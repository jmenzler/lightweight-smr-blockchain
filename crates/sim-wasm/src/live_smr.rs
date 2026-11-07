//! Interactive SMR session for the browser lab. It departs from the paper's model in these ways:
//!
//! - Blocking is chosen by a person who sees the round's start state: `step(fraction)` and
//!   `set_block` are not 1-late, so a session can target a server that holds a fresh command.
//! - `set_block` overlays hand-picked servers on the sampled mask without a beta budget, so a
//!   round can block more than the `fraction` passed to `step`. Such a session, and its export,
//!   lies outside Def 1.1 and Theorem 3 and must not be reported as a paper-model run.
//! - `inject` may pin a delivery target, where the paper's client picks a server at random.
//! - Client discipline is not enforced: the paper's client sends sn+1 only after sn is acked,
//!   but `inject` accepts a new command while an earlier one of the same client is unacked.
//!   A pipelining client can then lose hash chains, which `certs_json` shows as null chains.
//!   The lab form advances the client id after every submit, so it does not pipeline.
//! - The lab mounts the §5 certificate layer, which refuses execute-once, so it cannot run or replay
//!   a recovery scenario with `repeated_commit: skip`; both entries return an error for it. The
//!   compact rule accepts `skip`; recovery under `skip` is runnable only outside the lab.
//! - Step fractions replay exactly because the replay parses floats with `float_roundtrip`.

use crate::helpers::{
    chain_snapshot, check_fraction, error_json, merge_json, peaks_json, recorded_schedule,
};
use sim::smr::{
    CommandReport, ManualBlock, SmrReport, SmrScenario, SmrState, SmrStepStatus, SpreadCurve,
    run_smr,
};
use sim::spec::{InjectionSpec, SmrScenarioSpec, TrafficPhaseSpec, TrafficSpec};
use std::collections::{BTreeMap, HashMap};
use wasm_bindgen::prelude::wasm_bindgen;

/// Run a full SMR scenario from spec JSON; returns `{report}` or `{error}` JSON.
#[wasm_bindgen]
pub fn run_smr_json(spec_json: &str) -> String {
    match run_smr_inner(spec_json) {
        Ok(json) => json,
        Err(error) => serde_json::json!({ "error": error }).to_string(),
    }
}

fn run_smr_inner(spec_json: &str) -> Result<String, String> {
    let spec: SmrScenarioSpec = serde_json::from_str(spec_json).map_err(|e| e.to_string())?;
    let scenario = lab_scenario(spec)?;
    let report = run_smr(&scenario);
    assert_full_scope(&report);
    Ok(serde_json::json!({ "report": report }).to_string())
}

/// Certs mount on the parsed value, never the spec, so the same spec replays gated off byte-identically;
/// validating with them on refuses combinations (recovery with skip) the certificate layer cannot run.
fn lab_scenario(spec: SmrScenarioSpec) -> Result<SmrScenario, String> {
    let mut scenario = SmrScenario::try_from(spec)?;
    scenario.certs = true;
    scenario.validate()?;
    Ok(scenario)
}

fn assert_full_scope(report: &sim::smr::SmrReport) {
    assert!(
        report
            .commands
            .iter()
            .all(|c| c.spread.observed().is_some()),
        "the lab was handed a report whose position census was scoped out; the \
         curve would render as zero rather than as absent"
    );
}

/// What `report_delta_json` has already handed out.
#[derive(Default)]
struct DeltaCursor {
    rounds: usize,
    rec_rows: usize,
    commands: HashMap<u64, CommandReport>,
}

impl DeltaCursor {
    fn delta(&mut self, mut report: SmrReport) -> String {
        let metrics = std::mem::take(&mut report.metrics);
        let mut commands = std::mem::take(&mut report.commands);
        let rec_rows = report
            .recovery
            .as_mut()
            .map(|r| std::mem::take(&mut r.rounds))
            .unwrap_or_default();

        let mut inserted = Vec::new();
        let mut changed = Vec::new();
        let mut appended = Vec::new();
        for (index, command) in commands.iter().enumerate() {
            match self.commands.get(&command.op) {
                None => inserted.push(index),
                Some(sent) if sent == command => {}
                Some(sent) if only_grew(sent, command) => appended.push(index),
                Some(_) => changed.push(index),
            }
        }
        let appended_rows = self.appended_rows(&mut commands, &appended);
        // Removing the row lists from the serialized remainder keeps a future report field in the summary.
        let mut summary = serde_json::to_value(&report).expect("report serializes");
        let fields = summary.as_object_mut().expect("report is an object");
        fields.remove("metrics");
        fields.remove("commands");
        if let Some(recovery) = fields.get_mut("recovery").and_then(|r| r.as_object_mut()) {
            recovery.remove("rounds");
        }

        let delta = format!(
            r#"{{"rounds_from":{},"metrics":{},"rec_rows_from":{},"rec_rows":{},"summary":{},"len":{},"inserted":{},"changed":{},"appended":{}}}"#,
            self.rounds,
            serde_json::to_string(&metrics[self.rounds..]).expect("serializes"),
            self.rec_rows,
            serde_json::to_string(&rec_rows[self.rec_rows..]).expect("serializes"),
            summary,
            commands.len(),
            indexed(&commands, &inserted),
            indexed(&commands, &changed),
            appended_rows,
        );
        self.rounds = metrics.len();
        self.rec_rows = rec_rows.len();
        let mut sent = vec![false; commands.len()];
        for index in inserted.into_iter().chain(changed).chain(appended) {
            sent[index] = true;
        }
        for (command, sent) in commands.into_iter().zip(sent) {
            if sent {
                self.commands.insert(command.op, command);
            }
        }
        delta
    }
}

impl DeltaCursor {
    /// `[[index, command without its curve and attempts, curve length sent, new curve points,
    /// attempts length sent, new attempts], ...]`. The page overwrites the two lists on the scalars.
    fn appended_rows(&self, commands: &mut [CommandReport], indices: &[usize]) -> String {
        let rows: Vec<String> = indices
            .iter()
            .map(|&i| {
                let command = &mut commands[i];
                let sent = &self.commands[&command.op];
                let (spread_from, attempts_from) =
                    (sent.spread.points().len(), sent.attempts.len());
                let spread = std::mem::replace(&mut command.spread, SpreadCurve::NotObserved);
                let attempts = std::mem::take(&mut command.attempts);
                let row = format!(
                    "[{i},{},{spread_from},{},{attempts_from},{}]",
                    serde_json::to_string(&*command).expect("serializes"),
                    serde_json::to_string(&spread.points()[spread_from..]).expect("serializes"),
                    serde_json::to_string(&attempts[attempts_from..]).expect("serializes"),
                );
                command.spread = spread;
                command.attempts = attempts;
                row
            })
            .collect();
        format!("[{}]", rows.join(","))
    }
}

/// Decided against what was last sent, never assumed: only then can the page append the tails.
fn only_grew(sent: &CommandReport, now: &CommandReport) -> bool {
    now.spread.points().starts_with(sent.spread.points())
        && now.attempts.starts_with(&sent.attempts)
}

/// `[[index, command], ...]` serialized straight to text, without a `Value` copy of each command.
fn indexed(commands: &[CommandReport], indices: &[usize]) -> String {
    let rows: Vec<String> = indices
        .iter()
        .map(|&i| {
            format!(
                "[{i},{}]",
                serde_json::to_string(&commands[i]).expect("serializes")
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

/// Stepwise SMR session whose history and injections replay byte-identically via `run_smr_json`.
#[wasm_bindgen]
pub struct LiveSmr {
    state: SmrState,
    base: SmrScenarioSpec,
    fractions: Vec<f64>,
    last: Option<SmrStepStatus>,
    sticky: bool,
    background: f64,
    /// Sticky blocked set carried between steps; not the last round's full mask.
    mask: Vec<bool>,
    last_blocked: Vec<bool>,
    overlay: Vec<bool>,
    block_events: Vec<ManualBlock>,
    certs: Option<sim::certs::CertHarness>,
    certs_from_checkpoints: bool,
    captured: BTreeMap<u32, (u64, protocol::certificates::Certificate)>,
    sent: DeltaCursor,
}

#[wasm_bindgen]
impl LiveSmr {
    /// Spec's `schedule` is ignored: no up-front mask draw may touch the RNG stream.
    #[wasm_bindgen(constructor)]
    pub fn new(spec_json: &str) -> Result<LiveSmr, String> {
        LiveSmr::new_with_mode(spec_json, false)
    }

    /// Same sticky semantics as `LiveSim::new_with_mode`.
    pub fn new_with_mode(spec_json: &str, sticky: bool) -> Result<LiveSmr, String> {
        let spec: SmrScenarioSpec = serde_json::from_str(spec_json).map_err(|e| e.to_string())?;
        let scenario = lab_scenario(spec.clone())?;
        if !scenario.client_model.is_unique() {
            return Err("client_model pool is batch-only: the lab has no pool support".into());
        }
        if !scenario.manual_blocks.is_empty() {
            return Err("manual_blocks are batch-only: live blocking is set with set_block".into());
        }
        if scenario.halt_on_violation {
            return Err(
                "halt_on_violation is batch-only: a live session never halts itself".into(),
            );
        }
        let background = match (sticky, &scenario.schedule) {
            (true, sim::BlockSchedule::FreshPerRound { fraction }) => *fraction,
            _ => 0.0,
        };
        let mask = vec![false; scenario.n];
        // certs on here, never in `self.base`, so the exported spec replays gated off
        let state = SmrState::new_with_certs(
            scenario.n,
            scenario.cfg,
            scenario.proto,
            scenario.sigma,
            scenario.seed,
            &scenario.injections,
            scenario.traffic.as_deref().unwrap_or(&[]),
            scenario.client_model,
            scenario.partition.as_deref(),
            true,
            scenario.merge_policy,
            scenario.repeated_commit,
        );
        let certs_from_checkpoints = matches!(
            scenario.proto,
            sim::smr::Proto::Recovery {
                resend_until_acked: true,
                ..
            }
        );
        let has_certs =
            certs_from_checkpoints || matches!(scenario.proto, sim::smr::Proto::Compact { .. });
        let certs = has_certs.then(|| {
            let mut harness = sim::certs::CertHarness::new(scenario.n);
            let sns = sim::smr::injection_sns(&scenario.injections);
            for (inj, sn) in scenario.injections.iter().zip(sns) {
                harness.register_client(inj.client);
                harness.record_issue(inj.client, sn, inj.op);
            }
            harness
        });
        Ok(LiveSmr {
            state,
            base: spec,
            fractions: Vec::new(),
            last: None,
            sticky,
            background,
            overlay: vec![false; scenario.n],
            block_events: Vec::new(),
            mask,
            last_blocked: vec![false; scenario.n],
            certs,
            certs_from_checkpoints,
            captured: BTreeMap::new(),
            sent: DeltaCursor::default(),
        })
    }

    /// Advance one round at `fraction` blocking (an error JSON outside [0, 1]); no-op once dead so the replay length matches.
    pub fn step(&mut self, fraction: f64) -> String {
        if let Err(error) = check_fraction(fraction) {
            return error_json(error);
        }
        if let Some(last) = &self.last
            && (last.dead.is_some() || last.failed())
        {
            return status_json(last, true);
        }
        // stage (a0) precedes every mask draw so both arms stay on the batch stream
        self.state.draw_arrivals();
        let count = self.state.blocked_count(fraction);
        let background_count = self.state.blocked_count(self.background);
        let mut mask = if self.sticky {
            self.state
                .sticky_round_mask(&mut self.mask, count, background_count)
        } else {
            self.state.sample_mask(count)
        };
        // the overlay unions after every draw, so a manual block leaves the RNG stream untouched
        for (m, &blocked) in mask.iter_mut().zip(&self.overlay) {
            *m |= blocked;
        }
        let status = self.state.step_masked(&mask);
        self.last_blocked = mask;
        self.fractions.push(fraction);
        if !status.failed()
            && let Some(certs) = &mut self.certs
        {
            if self.certs_from_checkpoints {
                certs.observe_round_rec(&self.state);
            } else {
                certs.observe_round(&self.state);
            }
        }
        let json = status_json(&status, false);
        self.last = Some(status);
        json
    }

    fn failed_action(&self, action: &str) -> Option<String> {
        self.state.failure().map(|_| {
            serde_json::json!({
                "error": format!("failed session is immutable; cannot {action}")
            })
            .to_string()
        })
    }

    /// Submit a command live (lands next round); a pinned `target` deviates from the paper's random-server client.
    pub fn inject(&mut self, client: u32, op: u64, target: Option<u32>) -> String {
        if let Some(error) = self.failed_action("inject") {
            return error;
        }
        let landing = self.state.round() + 1;
        if self
            .state
            .manual_injections()
            .iter()
            .any(|i| i.client == client && i.round > landing)
        {
            return error_json(format!(
                "client {client} has a command scheduled after round {landing}; \
                 a live command would take its sequence number and the export would renumber it"
            ));
        }
        match self.state.inject(client, op, target) {
            Ok(round) => {
                if let Some(certs) = &mut self.certs {
                    certs.register_client(client);
                    let sn = certs.issued_count(client) + 1;
                    certs.record_issue(client, sn, op);
                }
                serde_json::json!({ "round": round }).to_string()
            }
            Err(error) => serde_json::json!({ "error": error }).to_string(),
        }
    }

    /// Block or release one node by hand; lands next round so the export still replays.
    pub fn set_block(&mut self, node: u32, blocked: bool) -> String {
        if let Some(error) = self.failed_action("change blocking") {
            return error;
        }
        let Some(slot) = self.overlay.get_mut(node as usize) else {
            return serde_json::json!({
                "error": format!("node {node} out of range for n={}", self.overlay.len())
            })
            .to_string();
        };
        let round = self.state.round() + 1;
        if *slot != blocked {
            *slot = blocked;
            if blocked {
                self.block_events.push(ManualBlock {
                    node,
                    from_round: round,
                    to_round: None,
                });
            } else if let Some(open) = self
                .block_events
                .iter()
                .rposition(|b| b.node == node && b.to_round.is_none())
            {
                if self.block_events[open].from_round == round {
                    self.block_events.remove(open);
                } else {
                    self.block_events[open].to_round = Some(round);
                }
            }
        }
        serde_json::json!({ "round": round }).to_string()
    }

    /// Live arrival-pmf edit; applies from the next round and replaces pending and future phases.
    pub fn set_traffic(&mut self, pmf_json: &str) -> String {
        if let Some(error) = self.failed_action("change traffic") {
            return error;
        }
        match serde_json::from_str::<Vec<f64>>(pmf_json)
            .map_err(|e| e.to_string())
            .and_then(|pmf| self.state.set_traffic_pmf(pmf))
        {
            Ok(()) => serde_json::json!({ "ok": true }).to_string(),
            Err(error) => serde_json::json!({ "error": error }).to_string(),
        }
    }

    /// The full report so far (metrics, per-command audit trails, terminal).
    pub fn report_json(&self) -> String {
        let report = self.state.report();
        assert_full_scope(&report);
        serde_json::to_string(&report).expect("report serializes")
    }

    /// What changed in the report since the previous call (the first call: since nothing), so a
    /// per-step message does not grow with the run. The page rebuilds `report_json()` from it.
    pub fn report_delta_json(&mut self) -> String {
        let report = self.state.report();
        assert_full_scope(&report);
        self.sent.delta(report)
    }

    pub fn node_detail(&self, node: u32) -> String {
        if let Some(error) = self.failed_action("read node detail") {
            return error;
        }
        match self.state.rec_node_detail(node as usize) {
            Some(detail) => serde_json::to_string(&detail).expect("detail serializes"),
            None => serde_json::json!({
                "error": "node detail needs the recovery rule and a node id below n"
            })
            .to_string(),
        }
    }

    /// The longest executed sequence over the n servers (a display construct, not any one server's S_i)
    /// as a Merkle forest snapshot, delta-encoded from `from_len`.
    pub fn chain_json(&self, from_len: u64) -> String {
        if let Some(error) = self.failed_action("read the committed chain") {
            return error;
        }
        let Some(entries) = self.state.committed_entries() else {
            return serde_json::json!({
                "error": "the committed chain needs the recovery rule (Alg 6)"
            })
            .to_string();
        };
        if !self.state.safety_ok() {
            return serde_json::json!({
                "error": "chain frozen: prefix consistency violated"
            })
            .to_string();
        }
        chain_snapshot(&entries, from_len)
    }

    /// A prefix-inconsistent population must not have its attestations drawn as sound.
    fn verifiable_certs(&self) -> Result<&sim::certs::CertHarness, String> {
        let certs = self.certs_or_error()?;
        if !self.state.safety_ok() {
            return Err("certificates frozen: prefix consistency violated".into());
        }
        Ok(certs)
    }

    fn certs_or_error(&self) -> Result<&sim::certs::CertHarness, String> {
        self.certs.as_ref().ok_or_else(|| {
            match self.base.proto {
                sim::spec::ProtoSpec::Recovery { .. } => {
                    "recovery certificates need the resend-until-acked client (RQ11)"
                }
                _ => "certificates need the compact rule (extended has no executed state)",
            }
            .to_string()
        })
    }

    /// Forest + certificate snapshot for the live drawer (pure read).
    pub fn certs_json(&self) -> String {
        if let Some(error) = self.failed_action("read certificates") {
            return error;
        }
        let certs = match self.verifiable_certs() {
            Ok(certs) => certs,
            Err(error) => return error_json(error),
        };
        let servers: Vec<serde_json::Value> = certs
            .servers()
            .enumerate()
            .map(|(i, s)| {
                serde_json::json!({
                    "server": i,
                    "m": s.forest().len(),
                    "peaks": peaks_json(s.forest()),
                })
            })
            .collect();
        let merges: Vec<serde_json::Value> = certs
            .merges_last_round()
            .iter()
            .map(|(server, e)| {
                let mut merge = merge_json(e);
                merge["server"] = serde_json::json!(server);
                merge
            })
            .collect();
        let clients: Vec<serde_json::Value> = certs
            .clients_iter()
            .map(|c| {
                let acked = c.next_sn() - 1;
                let rows: Vec<serde_json::Value> = (1..=c.issued_count())
                    .map(|sn| {
                        let (pos, chain_len) = match c.chain_for(sn) {
                            Some((pos, chain)) => {
                                (serde_json::json!(pos), serde_json::json!(chain.len()))
                            }
                            None => (serde_json::Value::Null, serde_json::Value::Null),
                        };
                        serde_json::json!({
                            "sn": sn,
                            "pos": pos,
                            "chain_len": chain_len,
                            "committed": sn <= acked,
                        })
                    })
                    .collect();
                let stale = match self.captured.get(&c.client()) {
                    Some((sn, cert)) => {
                        let t = certs.verify_everywhere(c.client(), cert, &self.last_blocked);
                        serde_json::json!({
                            "sn": sn,
                            "accepted": t.accepted,
                            "covered": t.covered,
                        })
                    }
                    None => serde_json::Value::Null,
                };
                serde_json::json!({
                    "client": c.client(),
                    "next_sn": c.next_sn(),
                    "certs": rows,
                    "stale": stale,
                })
            })
            .collect();
        // first max-m server: must match the lab's JS `best` tie-break (first wins)
        let best = certs
            .servers()
            .enumerate()
            .max_by_key(|(i, s)| (s.forest().len(), std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let committed: Vec<serde_json::Value> = self
            .state
            .executed_entries(best)
            .into_iter()
            .flat_map(protocol::shared_state::ExecSeq::iter)
            .map(|entry| match entry {
                protocol::compact::Entry::Cmd(cc) => {
                    serde_json::json!({ "kind": "cmd", "client": cc.client, "sn": cc.sn })
                }
                protocol::compact::Entry::Null { client, sn } => {
                    serde_json::json!({ "kind": "null", "client": client, "sn": sn })
                }
                protocol::compact::Entry::Nop(_) => serde_json::json!({ "kind": "nop" }),
            })
            .collect();
        serde_json::json!({
            "roots_consistent": certs.roots_consistent(),
            "servers": servers,
            "merges_last_step": merges,
            "clients": clients,
            "committed_server": best,
            "committed": committed,
        })
        .to_string()
    }

    /// Freeze the client's current bare-newest certificate for the §5 staleness demo.
    pub fn capture_stale(&mut self, client: u32) -> String {
        if let Some(error) = self.failed_action("capture a certificate") {
            return error;
        }
        let certs = match self.verifiable_certs() {
            Ok(certs) => certs,
            Err(error) => return error_json(error),
        };
        if certs.clients_iter().all(|c| c.client() != client) {
            return serde_json::json!({ "error": format!("unknown client {client}") }).to_string();
        }
        let acked = certs.acked_sn(client);
        if acked == 0 {
            return serde_json::json!({
                "error": format!("client {client} has no committed command to capture yet")
            })
            .to_string();
        }
        let cert = certs
            .client(client)
            .build_certificate(acked)
            .expect("issued commands keep their entry");
        self.captured.insert(client, (acked, cert));
        serde_json::json!({ "sn": acked }).to_string()
    }

    /// On-demand verify-everywhere tallies for every issued certificate of every client.
    pub fn verify_json(&self) -> String {
        if let Some(error) = self.failed_action("verify certificates") {
            return error;
        }
        let certs = match self.verifiable_certs() {
            Ok(certs) => certs,
            Err(error) => return error_json(error),
        };
        let round = self.last.as_ref().map(|s| s.round).unwrap_or(0);
        let clients: Vec<serde_json::Value> = certs
            .clients_iter()
            .map(|c| {
                let rows: Vec<serde_json::Value> = (1..=c.issued_count())
                    .map(|sn| {
                        let tally = match c.build_certificate(sn) {
                            Some(cert) => {
                                let t =
                                    certs.verify_everywhere(c.client(), &cert, &self.last_blocked);
                                serde_json::json!({
                                    "accepted": t.accepted,
                                    "covered": t.covered,
                                })
                            }
                            None => serde_json::Value::Null,
                        };
                        serde_json::json!({ "sn": sn, "tally": tally })
                    })
                    .collect();
                serde_json::json!({ "client": c.client(), "rows": rows })
            })
            .collect();
        serde_json::json!({ "round": round, "clients": clients }).to_string()
    }

    pub fn export_scenario(&self) -> String {
        let mut spec = self.base.clone();
        spec.injections = self
            .state
            .manual_injections()
            .iter()
            .map(|i| InjectionSpec {
                round: i.round,
                client: i.client,
                op: i.op,
                target: i.target,
            })
            .collect();
        spec.traffic = match self.state.traffic_phases() {
            [] => None,
            [only] if only.from_round == 1 => Some(TrafficSpec::Pmf {
                arrivals_pmf: only.arrivals_pmf.clone(),
            }),
            phases => Some(TrafficSpec::Phases {
                phases: phases
                    .iter()
                    .map(|p| TrafficPhaseSpec {
                        from_round: p.from_round,
                        arrivals_pmf: p.arrivals_pmf.clone(),
                    })
                    .collect(),
            }),
        };
        spec.schedule = Some(recorded_schedule(
            self.sticky,
            &self.fractions,
            self.background,
        ));
        spec.manual_blocks = self.block_events.clone();
        spec.max_rounds = self.fractions.len();
        serde_json::to_string(&spec).expect("spec serializes")
    }
}

#[derive(serde::Serialize)]
struct StatusOut<'a> {
    #[serde(flatten)]
    status: &'a SmrStepStatus,
    absorbed: bool,
}

fn status_json(status: &SmrStepStatus, absorbed: bool) -> String {
    serde_json::to_string(&StatusOut { status, absorbed }).expect("status serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r#"{"n":32,"seed":1,"k":6,"ell":3,"sigma":1.5,
        "proto":{"kind":"recovery","t_window_rounds":10,"resend_until_acked":true},
        "injections":[{"round":2,"client":1,"op":7}],"max_rounds":400,
        "schedule":{"kind":"fresh_per_round","fraction":0.1},
        "traffic":{"kind":"pmf","arrivals_pmf":[0.3,0.4,0.3]}}"#;

    fn indices(delta: &serde_json::Value, key: &str) -> Vec<u64> {
        delta[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[0].as_u64().unwrap())
            .collect()
    }

    fn status_value_reference(status: &SmrStepStatus, absorbed: bool) -> serde_json::Value {
        let mut value = serde_json::to_value(status).expect("status serializes");
        value["absorbed"] = serde_json::json!(absorbed);
        value
    }

    #[test]
    fn status_json_matches_the_value_tree_it_replaced() {
        for proto in [
            r#"{"kind":"extended"}"#,
            r#"{"kind":"compact","t_commit_rounds":6}"#,
            r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":true}"#,
        ] {
            let spec = SPEC.replace(
                r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":true}"#,
                proto,
            );
            let mut live = LiveSmr::new(&spec).unwrap();
            for _ in 0..40 {
                live.step(0.1);
                let status = live.last.as_ref().unwrap();
                for absorbed in [false, true] {
                    let got: serde_json::Value =
                        serde_json::from_str(&status_json(status, absorbed)).unwrap();
                    assert_eq!(got, status_value_reference(status, absorbed));
                }
            }
        }
    }

    #[test]
    fn a_change_that_is_not_a_prefix_extension_goes_out_whole() {
        let mut live = LiveSmr::new(SPEC).unwrap();
        for _ in 0..20 {
            live.step(0.3);
        }
        let mut cursor = DeltaCursor::default();
        cursor.delta(live.state.report());
        for _ in 0..3 {
            live.step(0.3);
        }
        let mut report = live.state.report();
        let rewritten = report
            .commands
            .iter()
            .position(|c| c.spread.points().len() >= 2)
            .expect("a command with a curve");
        let SpreadCurve::Observed(points) = &mut report.commands[rewritten].spread else {
            unreachable!("full scope");
        };
        points.remove(0);

        let delta: serde_json::Value = serde_json::from_str(&cursor.delta(report)).unwrap();
        assert!(indices(&delta, "changed").contains(&(rewritten as u64)));
        assert!(!indices(&delta, "appended").contains(&(rewritten as u64)));
        assert!(
            !indices(&delta, "appended").is_empty(),
            "the other changed commands still go out as tails"
        );
    }
}
