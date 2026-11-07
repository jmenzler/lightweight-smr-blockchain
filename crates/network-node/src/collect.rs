//! Pure aggregation of per-node round records into sim-schema CSVs + convergence summary.

use crate::record::{PostState, RoundRecord};
use crate::spec::NodeRunSpec;
use sim::runlog::{SMR_METRICS_CSV_HEADER, VALUE_METRICS_CSV_HEADER};
use std::collections::{BTreeMap, BTreeSet};

const MESSAGES_CSV_HEADER: &str =
    "round,requests,replies,appends,client_msgs,acks,bytes_out,bytes_in";

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// First round of the final unbroken unanimity streak (it restarts whenever unanimity breaks or
    /// the unanimous value changes) on the value the rule must spread: x for gossip, the maximum
    /// initial value for priority, any value for median. The simulator's `agreed_since` is the first
    /// round of agreement and is never reset, and it reports an all-⊥ round as `AllUndecided`; the two
    /// differ only on a run that agrees and later loses its holders (how long agreement must last is
    /// open, M.OPEN.15, p. 19). `None` covers both a horizon without unanimity and an all-⊥ horizon,
    /// which the paper counts as vacuous agreement plus an availability failure (Cor. 2.10, p. 19);
    /// the `undecided` column of metrics.csv tells them apart.
    pub convergence_round: Option<u64>,
    /// First round of persistent unanimity on any value, and that value.
    pub agreed_round: Option<u64>,
    pub agreed_value: Option<u64>,
    /// The value gossip/priority must spread; `None` for the median arm.
    pub target_value: Option<u64>,
    /// Per round, how many servers hold `target_value`; empty without a target.
    pub target_holders: Vec<u32>,
    /// Round-timing tails across all nodes × rounds.
    pub timing: RoundTiming,
}

/// Max + nearest-rank p99 of the per-round timing fields, in microseconds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoundTiming {
    pub t_last_reply_us_max: u64,
    pub t_last_reply_us_p99: u64,
    pub t_step_done_us_max: u64,
    pub t_step_done_us_p99: u64,
    pub t_step_compute_us_max: u64,
    pub t_step_compute_us_p99: u64,
}

fn timing_stats(records: &[Vec<RoundRecord>]) -> RoundTiming {
    fn tail(mut vals: Vec<u64>) -> (u64, u64) {
        if vals.is_empty() {
            return (0, 0);
        }
        vals.sort_unstable();
        let p99 = vals[(vals.len() * 99).div_ceil(100) - 1];
        (*vals.last().expect("non-empty"), p99)
    }
    let all =
        |f: fn(&RoundRecord) -> u64| -> Vec<u64> { records.iter().flatten().map(f).collect() };
    let (reply_max, reply_p99) = tail(all(|r| r.t_last_reply_us));
    let (step_max, step_p99) = tail(all(|r| r.t_step_done_us));
    let (compute_max, compute_p99) = tail(all(|r| r.t_step_compute_us));
    RoundTiming {
        t_last_reply_us_max: reply_max,
        t_last_reply_us_p99: reply_p99,
        t_step_done_us_max: step_max,
        t_step_done_us_p99: step_p99,
        t_step_compute_us_max: compute_max,
        t_step_compute_us_p99: compute_p99,
    }
}

/// Validity gates; a run is only sim-comparable when `clean()`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gates {
    pub late_rep: u64,
    pub late_app: u64,
    pub dropped_past: u64,
    pub late_cli: u64,
    pub early_cli: u64,
    pub encode_err: u64,
    pub transport_err: u64,
    /// Client attempts that failed in connect, write, read or decode: a message the model does not lose.
    pub client_lost: u64,
    /// Client attempts cut off by their round deadline before they finished.
    pub client_starved: u64,
    /// |client sends − commands the servers received, found late, found early or blocked| over the run.
    pub cli_unreconciled: u64,
    /// |sent − received| over the whole run; blocked recipients and late arrivals are accounted for.
    pub req_unreconciled: u64,
    pub rep_unreconciled: u64,
    pub app_unreconciled: u64,
    /// Every node recorded exactly `max_rounds` rounds; a run cut short by a host stop fails it.
    pub rounds_consistent: bool,
    /// Protocol-side check, latched false on the first violation. Compact: two servers executed
    /// different entries at the same executed length (prefix consistency across all servers,
    /// `⊥` ones included, which implies the per-server monotonicity of Def 1.5). Not Def 1.4 Strong
    /// Safety (equality among useful servers): a lagging server passes. Value modes: a server
    /// holds a value that no server started with. Extended has no execution layer and no check,
    /// so it stays true. A record flagged `sn_order_violation` also latches it false. Hashes are 64-bit FNV, so a collision (about m²/2^64 for m distinct
    /// prefixes) could hide a divergence.
    pub safety_ok: bool,
}

impl Gates {
    /// Transport and bookkeeping gates only: whether the run measured the model, not whether the
    /// protocol held in it.
    pub fn network_clean(&self) -> bool {
        self.late_rep == 0
            && self.late_app == 0
            && self.dropped_past == 0
            && self.late_cli == 0
            && self.early_cli == 0
            && self.encode_err == 0
            && self.transport_err == 0
            && self.client_lost == 0
            && self.client_starved == 0
            && self.req_unreconciled == 0
            && self.rep_unreconciled == 0
            && self.app_unreconciled == 0
            && self.cli_unreconciled == 0
            && self.rounds_consistent
    }

    pub fn clean(&self) -> bool {
        self.network_clean() && self.safety_ok
    }

    fn accumulate(&mut self, rec: &RoundRecord) {
        self.late_rep += u64::from(rec.late_rep);
        self.late_app += u64::from(rec.late_app);
        self.dropped_past += u64::from(rec.dropped_past);
        self.late_cli += u64::from(rec.late_cli);
        self.early_cli += u64::from(rec.early_cli);
        self.encode_err += u64::from(rec.encode_err);
        self.transport_err += u64::from(rec.transport_err);
    }

    fn reconcile(&mut self, flow: &Flow, client: Option<&ClientLog>) {
        self.req_unreconciled = flow.req_sent.abs_diff(flow.req_recv);
        self.rep_unreconciled = flow.rep_sent.abs_diff(flow.rep_recv + flow.late_rep);
        self.app_unreconciled = flow
            .app_sent
            .abs_diff(flow.app_recv + flow.app_blocked + flow.late_app);
        if let Some(client) = client {
            self.client_lost = client.lost;
            self.client_starved = client.starved;
            self.cli_unreconciled = client.sent.abs_diff(flow.cli_accounted);
        }
    }
}

/// The client driver's stdout: per-round arrivals and failure counts, then the final command table.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientLog {
    pub arrivals: Vec<u32>,
    pub lost: u64,
    pub starved: u64,
    /// Attempts whose command left the client, to be matched against what the servers counted.
    pub sent: u64,
    /// The `{"commands":[...]}` line, verbatim.
    pub commands: String,
}

impl ClientLog {
    pub fn parse(stdout: &str) -> Result<ClientLog, String> {
        let mut log = ClientLog {
            arrivals: Vec::new(),
            lost: 0,
            starved: 0,
            sent: 0,
            commands: String::from("{\"commands\":[]}"),
        };
        for line in stdout.lines() {
            let value: serde_json::Value =
                serde_json::from_str(line).map_err(|e| format!("client line {line:?}: {e}"))?;
            let count = |key: &str| value.get(key).and_then(serde_json::Value::as_u64);
            if let Some(a) = count("arrivals") {
                log.arrivals
                    .push(u32::try_from(a).map_err(|e| format!("arrivals: {e}"))?);
            } else if let Some(l) = count("lost") {
                log.lost += l;
            } else if let Some(s) = count("starved") {
                log.starved += s;
            } else if let Some(s) = count("sent") {
                log.sent += s;
            } else if value.get("commands").is_some() {
                log.commands = line.to_string();
            } else {
                return Err(format!("unrecognised client line {line:?}"));
            }
        }
        Ok(log)
    }
}

/// Run-wide message flow: every message sent must be received, counted late, or hit a blocked server.
#[derive(Default)]
struct Flow {
    req_sent: u64,
    req_recv: u64,
    rep_sent: u64,
    rep_recv: u64,
    late_rep: u64,
    app_sent: u64,
    app_recv: u64,
    app_blocked: u64,
    late_app: u64,
    /// Client commands that reached a server, whatever became of them there.
    cli_accounted: u64,
}

impl Flow {
    fn add(&mut self, rec: &RoundRecord) {
        self.req_sent += u64::from(rec.req_sent);
        self.req_recv += u64::from(rec.req_recv);
        self.rep_sent += u64::from(rec.rep_sent);
        self.rep_recv += u64::from(rec.rep_recv);
        self.late_rep += u64::from(rec.late_rep);
        self.app_sent += u64::from(rec.app_sent);
        self.app_recv += u64::from(rec.app_recv);
        self.app_blocked += u64::from(rec.app_blocked);
        self.late_app += u64::from(rec.late_app);
        self.cli_accounted += u64::from(rec.cli_recv)
            + u64::from(rec.late_cli)
            + u64::from(rec.early_cli)
            + u64::from(rec.cli_blocked);
    }
}

#[derive(Default)]
struct MsgTotals {
    req: u64,
    rep: u64,
    app: u64,
    cli: u64,
    acks: u64,
    bytes_out: u64,
    bytes_in: u64,
}

impl MsgTotals {
    fn add(&mut self, rec: &RoundRecord) {
        self.req += u64::from(rec.req_sent);
        self.rep += u64::from(rec.rep_recv);
        self.app += u64::from(rec.app_recv);
        self.cli += u64::from(rec.cli_recv);
        self.acks += u64::from(rec.acks);
        self.bytes_out += rec.bytes_out;
        self.bytes_in += rec.bytes_in;
    }

    fn csv_row(&self, round: usize) -> String {
        format!(
            "{round},{},{},{},{},{},{},{}\n",
            self.req, self.rep, self.app, self.cli, self.acks, self.bytes_out, self.bytes_in
        )
    }
}

/// Per tracked op, the sim's SMR landmarks derived from the per-round digests.
#[derive(Debug, Clone, PartialEq)]
pub struct Landmark {
    pub op: u64,
    /// T_B — first round the op is in EVERY non-⊥ log; once a server has committed it, it has left that
    /// server's log, as in the simulator's coverage latch.
    pub all_logs_round: Option<u64>,
    /// T_E as Lemma 3.8 reads it — first round all non-⊥ nodes agree on its position and prefix,
    /// persisting to the horizon. Never earlier than Def 1.3's agreement on the position alone.
    /// Rounds in which every node is ⊥ are skipped, so a run that dies after T_E keeps reporting it as
    /// settled (the simulator's extended tracker does the same). Agreement compares (position, 64-bit
    /// FNV prefix hash) pairs. The simulator reports no T_E for compact runs.
    pub prefix_fixed_round: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Collected {
    /// Byte-compatible with the sim runlog CSV schema, except the extended arm's empty `lcp_len`.
    pub metrics_csv: String,
    pub messages_csv: String,
    pub summary: Summary,
    pub gates: Gates,
    /// SMR only; empty for the value modes.
    pub landmarks: Vec<Landmark>,
}

/// SMR aggregation into the sim's 10-column schema.
pub fn aggregate_smr(
    records: &[Vec<RoundRecord>],
    spec: &NodeRunSpec,
    client: Option<&ClientLog>,
) -> Result<Collected, String> {
    // extended has no derivable LCP: an empty cell, never a 0 that reads as computed
    let compact = matches!(
        &spec.scenario,
        crate::spec::ScenarioKind::Smr(s)
            if matches!(s.proto, sim::spec::ProtoSpec::Compact { .. })
    );
    if records.is_empty() || records.iter().all(|r| r.is_empty()) {
        return Err("no records to aggregate".to_string());
    }
    let rounds = records.iter().map(|r| r.len()).max().unwrap_or(0);
    let rounds_consistent = records.iter().all(|r| r.len() == spec.max_rounds());

    let mut metrics_csv = format!("{SMR_METRICS_CSV_HEADER}\n");
    let mut messages_csv = format!("{MESSAGES_CSV_HEADER}\n");
    let mut gates = Gates {
        rounds_consistent,
        safety_ok: true,
        ..Gates::default()
    };
    let mut flow = Flow::default();
    let mut prefix_witness: BTreeMap<usize, u64> = BTreeMap::new();
    let mut tracked_ops: Vec<u64> = Vec::new();
    let mut tb: Vec<Option<u64>> = Vec::new();
    let mut te: Vec<Option<u64>> = Vec::new();

    for i in 0..rounds {
        let round_recs: Vec<&RoundRecord> = records.iter().filter_map(|r| r.get(i)).collect();
        let mut nonbot = 0u32;
        let mut blocked = 0u32;
        let mut useful = 0u32;
        let mut hashes = BTreeSet::new();
        let mut max_log_len = 0usize;
        let mut exec_lens = Vec::new();
        let mut totals = MsgTotals::default();
        for rec in &round_recs {
            match &rec.post {
                PostState::Log {
                    log_len,
                    exec_len,
                    log_hash,
                } => {
                    if let Some(len) = log_len {
                        nonbot += 1;
                        max_log_len = max_log_len.max(*len);
                    }
                    if let Some(h) = log_hash {
                        hashes.insert(*h);
                    }
                    exec_lens.push(*exec_len);
                }
                PostState::Value(_) => {
                    return Err("value-mode records need aggregate(), not aggregate_smr()".into());
                }
            }
            blocked += u32::from(rec.blocked);
            useful += u32::from(rec.snap_held && !rec.blocked);
            totals.add(rec);
            flow.add(rec);
            gates.accumulate(rec);
            gates.safety_ok &= !rec.sn_order_violation;
            for &(len, hash) in &rec.exec_prefix {
                match prefix_witness.get(&len) {
                    Some(&seen) if seen != hash => gates.safety_ok = false,
                    Some(_) => {}
                    None => {
                        prefix_witness.insert(len, hash);
                    }
                }
            }
            for d in &rec.tracked {
                if !tracked_ops.contains(&d.op) {
                    tracked_ops.push(d.op);
                    tb.push(None);
                    te.push(None);
                }
            }
        }
        let round = i as u64 + 1;
        let live: Vec<&RoundRecord> = round_recs
            .iter()
            .copied()
            .filter(|r| {
                matches!(
                    &r.post,
                    PostState::Log {
                        log_len: Some(_),
                        ..
                    }
                )
            })
            .collect();
        for (idx, op) in tracked_ops.iter().enumerate() {
            let digests: Vec<&crate::record::TrackedDigest> = live
                .iter()
                .filter_map(|r| r.tracked.iter().find(|d| d.op == *op))
                .collect();
            if digests.is_empty() {
                continue;
            }
            if digests.iter().all(|d| d.in_log) {
                tb[idx].get_or_insert(round);
            }
            if digests.iter().all(|d| d.present) {
                let first = (digests[0].pos, digests[0].prefix_hash);
                let agreed =
                    first.0.is_some() && digests.iter().all(|d| (d.pos, d.prefix_hash) == first);
                match (agreed, te[idx]) {
                    (true, None) => te[idx] = Some(round),
                    (false, Some(_)) => te[idx] = None,
                    _ => {}
                }
            } else if te[idx].is_some() {
                te[idx] = None;
            }
        }
        let min_exec = exec_lens.iter().min().copied().unwrap_or(0);
        let max_exec = exec_lens.iter().max().copied().unwrap_or(0);
        let arr = client.and_then(|c| c.arrivals.get(i)).copied().unwrap_or(0);
        let lcp = if compact {
            min_exec.to_string()
        } else {
            String::new()
        };
        metrics_csv.push_str(&format!(
            "{},{nonbot},{blocked},{useful},{},{max_log_len},{min_exec},{max_exec},{arr},{lcp}\n",
            i + 1,
            hashes.len()
        ));
        messages_csv.push_str(&totals.csv_row(i + 1));
    }

    gates.reconcile(&flow, client);
    let landmarks = tracked_ops
        .iter()
        .enumerate()
        .map(|(idx, &op)| Landmark {
            op,
            all_logs_round: tb[idx],
            prefix_fixed_round: te[idx],
        })
        .collect();
    Ok(Collected {
        metrics_csv,
        messages_csv,
        summary: Summary {
            convergence_round: None,
            agreed_round: None,
            agreed_value: None,
            target_value: None,
            target_holders: Vec::new(),
            timing: timing_stats(records),
        },
        gates,
        landmarks,
    })
}

pub fn aggregate(records: &[Vec<RoundRecord>], spec: &NodeRunSpec) -> Result<Collected, String> {
    if records.is_empty() || records.iter().all(|r| r.is_empty()) {
        return Err("no records to aggregate".to_string());
    }
    let rounds = records.iter().map(|r| r.len()).max().unwrap_or(0);
    let rounds_consistent = records.iter().all(|r| r.len() == spec.max_rounds());

    let mut metrics_csv = format!("{VALUE_METRICS_CSV_HEADER}\n");
    let mut messages_csv = format!("{MESSAGES_CSV_HEADER}\n");
    let mut gates = Gates {
        rounds_consistent,
        safety_ok: true,
        ..Gates::default()
    };
    let target = spec.target_value()?;
    let initial: BTreeSet<u64> = spec.initial_values()?.into_iter().flatten().collect();
    let mut target_holders: Vec<u32> = Vec::new();
    let mut flow = Flow::default();
    let mut candidate: Option<(u64, u64)> = None;

    for i in 0..rounds {
        let round_recs: Vec<&RoundRecord> = records.iter().filter_map(|r| r.get(i)).collect();
        let mut holders = 0u32;
        let mut undecided = 0u32;
        let mut blocked = 0u32;
        let mut useful = 0u32;
        let mut values = BTreeSet::new();
        let mut totals = MsgTotals::default();
        for rec in &round_recs {
            match &rec.post {
                PostState::Value(Some(v)) => {
                    holders += 1;
                    values.insert(*v);
                    gates.safety_ok &= initial.contains(v);
                }
                PostState::Value(None) => undecided += 1,
                PostState::Log { .. } => {
                    return Err("SMR aggregation lands with the SMR engine arms".to_string());
                }
            }
            blocked += u32::from(rec.blocked);
            useful += u32::from(rec.snap_held && !rec.blocked);
            totals.add(rec);
            flow.add(rec);
            gates.accumulate(rec);
        }
        if let Some(x) = target {
            target_holders.push(
                round_recs
                    .iter()
                    .filter(|r| r.post == PostState::Value(Some(x)))
                    .count() as u32,
            );
        }
        metrics_csv.push_str(&format!(
            "{},{holders},{undecided},{blocked},{useful},{}\n",
            i + 1,
            values.len()
        ));
        messages_csv.push_str(&totals.csv_row(i + 1));

        let unanimous = holders > 0 && values.len() == 1;
        let value = values.first().copied();
        candidate = match (unanimous, candidate) {
            (true, None) => Some((i as u64 + 1, value.expect("unanimous"))),
            (true, Some((_, v))) if value != Some(v) => {
                Some((i as u64 + 1, value.expect("unanimous")))
            }
            (true, keep) => keep,
            (false, _) => None,
        };
    }

    gates.reconcile(&flow, None);
    Ok(Collected {
        metrics_csv,
        messages_csv,
        summary: Summary {
            convergence_round: candidate
                .filter(|&(_, v)| target.is_none_or(|x| x == v))
                .map(|(r, _)| r),
            agreed_round: candidate.map(|(r, _)| r),
            agreed_value: candidate.map(|(_, v)| v),
            target_value: target,
            target_holders,
            timing: timing_stats(records),
        },
        gates,
        landmarks: Vec::new(),
    })
}

/// `summary.json` body shared by every bin that writes collector results.
pub fn summary_json(out: &Collected) -> String {
    let opt = |v: Option<u64>| v.map_or("null".into(), |v| v.to_string());
    let holders: Vec<String> = out
        .summary
        .target_holders
        .iter()
        .map(u32::to_string)
        .collect();
    format!(
        "{{\"convergence_round\":{},\"agreed_round\":{},\"agreed_value\":{},\"target_value\":{},\"target_holders\":[{}],\"timing\":{{\"t_last_reply_us_max\":{},\"t_last_reply_us_p99\":{},\"t_step_done_us_max\":{},\"t_step_done_us_p99\":{},\"t_step_compute_us_max\":{},\"t_step_compute_us_p99\":{}}},\"gates\":{{\"late_rep\":{},\"late_app\":{},\"dropped_past\":{},\"late_cli\":{},\"early_cli\":{},\"encode_err\":{},\"transport_err\":{},\"client_lost\":{},\"client_starved\":{},\"req_unreconciled\":{},\"rep_unreconciled\":{},\"app_unreconciled\":{},\"cli_unreconciled\":{},\"rounds_consistent\":{},\"safety_ok\":{}}}}}",
        opt(out.summary.convergence_round),
        opt(out.summary.agreed_round),
        opt(out.summary.agreed_value),
        opt(out.summary.target_value),
        holders.join(","),
        out.summary.timing.t_last_reply_us_max,
        out.summary.timing.t_last_reply_us_p99,
        out.summary.timing.t_step_done_us_max,
        out.summary.timing.t_step_done_us_p99,
        out.summary.timing.t_step_compute_us_max,
        out.summary.timing.t_step_compute_us_p99,
        out.gates.late_rep,
        out.gates.late_app,
        out.gates.dropped_past,
        out.gates.late_cli,
        out.gates.early_cli,
        out.gates.encode_err,
        out.gates.transport_err,
        out.gates.client_lost,
        out.gates.client_starved,
        out.gates.req_unreconciled,
        out.gates.rep_unreconciled,
        out.gates.app_unreconciled,
        out.gates.cli_unreconciled,
        out.gates.rounds_consistent,
        out.gates.safety_ok
    )
}
