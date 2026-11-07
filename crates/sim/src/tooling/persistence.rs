//! Split-brain persistence probe: blast radius and transience of commitment errors.
//!
//! A split is the cross-server half of no-fork only. Per-server Def 1.5 retractions are
//! counted separately and in every run, split or not. A "heal" (return to cross-server
//! consistency) under Alg 5 needs some server to retract executed commands, a further Def 1.5
//! violation, so every heal is reported with whether its episode retracted (`heals_by_retraction`)
//! and with the retractions of the first heal round (`r_h_retractions`).

use crate::BlockSchedule;
use crate::smr::{
    ClientModel, CommandStatus, Proto, SmrReport, SmrScenario, SmrState, SmrTerminal,
    prefixes_consistent,
};
use crate::spec::SmrScenarioSpec;
use protocol::compact::Entry;
use protocol::shared_state::ExecSeq;
use std::collections::{HashMap, HashSet};

/// Registered observation multiples m (read at r_v + m·T); changing the set changes the estimand.
pub const OBS_MULTIPLES: [usize; 5] = [0, 1, 2, 3, 10];

#[derive(Debug, Clone, Default)]
pub struct ProbeOptions {
    pub stop_rounds_after_violation: Option<usize>,
    /// Committed `safety_ok` this seed must reproduce; a mismatch is reported, never panicked.
    pub expected_safety: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Transience {
    /// First round after r_v that is cross-server consistent again; not a return to a valid
    /// state, since reaching it retracts executed commands (see [`Heals`]).
    pub r_h: Option<usize>,
    pub heal_rounds: Option<usize>,
    pub episodes: usize,
    pub inconsistent_rounds: usize,
    pub phi: f64,
}

pub fn transience(r_v: usize, split: &[bool]) -> Transience {
    let r_h = split.iter().position(|&s| !s).map(|i| r_v + i);
    Transience {
        r_h,
        heal_rounds: r_h.map(|r| r - r_v),
        episodes: split
            .iter()
            .enumerate()
            .filter(|&(i, &s)| s && (i == 0 || !split[i - 1]))
            .count(),
        inconsistent_rounds: split.iter().filter(|&&s| s).count(),
        phi: split.iter().filter(|&&s| s).count() as f64 / split.len() as f64,
    }
}

/// Returns to consistency after r_v and the per-server retractions they took.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Heals {
    /// Rounds after r_v that end a split episode.
    pub heals: usize,
    /// The heals with at least one retraction in their episode's healing span: the rounds after
    /// the one in which the split appeared, through the heal round. A retraction in the split's
    /// first round did not heal it, since the split held at that round's end.
    pub by_retraction: usize,
    /// Retractions in round r_h; `None` without a heal.
    pub r_h_retractions: Option<u64>,
}

/// `split` and `retractions` both start at r_v, one entry per round.
pub fn heals(split: &[bool], retractions: &[u64]) -> Heals {
    assert_eq!(
        split.len(),
        retractions.len(),
        "one entry per round after r_v"
    );
    let ends: Vec<usize> = (1..split.len())
        .filter(|&i| split[i - 1] && !split[i])
        .collect();
    let episode_start = |end: usize| {
        (0..end)
            .rev()
            .find(|&i| i == 0 || !split[i - 1])
            .expect("a heal follows a split round")
    };
    Heals {
        heals: ends.len(),
        by_retraction: ends
            .iter()
            .filter(|&&end| {
                retractions[episode_start(end) + 1..=end]
                    .iter()
                    .any(|&r| r > 0)
            })
            .count(),
        r_h_retractions: split.iter().position(|&s| !s).map(|i| retractions[i]),
    }
}

pub fn forked_branch(seqs: &[&[Entry]]) -> Option<usize> {
    let branches = partition_branches(seqs);
    let lead = seqs[branches.first()?[0]];
    branches
        .iter()
        .map(|members| members[0])
        .find(|&rep| !prefixes_consistent(&[lead, seqs[rep]]))
}

pub fn mutual_fork_pair(seqs: &[&[Entry]]) -> Option<(usize, usize)> {
    let branches = partition_branches(seqs);
    branches.iter().enumerate().find_map(|(i, x)| {
        branches[i + 1..]
            .iter()
            .find(|y| !prefixes_consistent(&[seqs[x[0]], seqs[y[0]]]))
            .map(|y| (x[0], y[0]))
    })
}

pub fn sn_regressions(seq: &[Entry]) -> usize {
    let mut first_op: HashMap<(u32, u64), u64> = HashMap::new();
    let mut conflicting: HashSet<(u32, u64)> = HashSet::new();
    for entry in seq {
        let Entry::Cmd(c) = entry else { continue };
        let key = (c.client, c.sn);
        match first_op.get(&key) {
            Some(&op) if op != c.op => {
                conflicting.insert(key);
            }
            Some(_) => {}
            None => {
                first_op.insert(key, c.op);
            }
        }
    }
    conflicting.len()
}

pub fn max_duplicate_executions(seqs: &[&[Entry]]) -> usize {
    worst_server(seqs, duplicate_executions)
}

pub fn max_sn_regressions(seqs: &[&[Entry]]) -> usize {
    worst_server(seqs, sn_regressions)
}

fn worst_server(seqs: &[&[Entry]], count: fn(&[Entry]) -> usize) -> usize {
    partition_branches(seqs)
        .iter()
        .map(|members| count(seqs[members[0]]))
        .max()
        .unwrap_or(0)
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeSpec {
    pub exp: String,
    pub base: SmrScenarioSpec,
    pub seeds: Vec<u64>,
    #[serde(default)]
    pub expect_violating: Vec<u64>,
    #[serde(default)]
    pub stop_rounds_after_violation: Option<usize>,
}

impl ProbeSpec {
    pub fn expand(&self) -> Result<Vec<(SmrScenario, ProbeOptions)>, String> {
        self.seeds
            .iter()
            .map(|&seed| {
                let mut spec = self.base.clone();
                spec.seed = seed;
                let scenario = SmrScenario::try_from(spec)?;
                let opts = ProbeOptions {
                    stop_rounds_after_violation: self.stop_rounds_after_violation,
                    expected_safety: self.expect_violating.contains(&seed).then_some(false),
                };
                Ok((scenario, opts))
            })
            .collect()
    }
}

/// Per-run detail path; the `-runs` marker keeps it from overwriting a `.json` spec.
pub fn detail_path(out: &str) -> String {
    format!("{}-runs.json", out.strip_suffix(".csv").unwrap_or(out))
}

pub fn parse_probe_specs(text: &str) -> Result<Vec<ProbeSpec>, String> {
    if text.trim_start().starts_with('[') {
        serde_json::from_str(text).map_err(|e| e.to_string())
    } else {
        serde_json::from_str::<ProbeSpec>(text)
            .map(|s| vec![s])
            .map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Observation {
    pub round: usize,
    pub branches: usize,
    pub inconsistent_nodes: usize,
    pub branch1: usize,
    pub branch2: usize,
    pub exec_len_a: usize,
    pub exec_len_b: usize,
    pub common: usize,
    pub d: usize,
    /// Largest branch disagreeing with A; `None` (not zero) when nothing contradicts A.
    pub fork_branch: Option<usize>,
    pub d_fork: Option<usize>,
    pub d_mutual: Option<usize>,
    pub first_mismatch: Option<usize>,
    pub last_mismatch: Option<usize>,
    pub displaced_after_rv: usize,
    pub dup_exec: usize,
    pub sn_regress: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeRun {
    #[serde(skip)]
    pub report: SmrReport,
    pub rounds: usize,
    pub truncated: bool,
    pub latch_round: Option<usize>,
    pub r_v: Option<usize>,
    pub transience: Option<Transience>,
    pub split_after_rv: Vec<bool>,
    /// Per-server Def 1.5 retractions in each round from r_v on, aligned with `split_after_rv`.
    pub retractions_after_rv: Vec<u64>,
    pub heals: Option<Heals>,
    /// Rounds in which some server's executed length dropped; rewrites are not counted here.
    pub shrink_events: usize,
    pub exec_len_rv: Option<usize>,
    pub observations: Vec<Observation>,
    pub expected_safety: Option<bool>,
    pub replay_ok: Option<bool>,
}

/// Replay one scenario under the reference oracle; draws nothing itself, so it matches `run_smr` (guarded).
pub fn probe_run(scenario: &SmrScenario, opts: &ProbeOptions) -> Result<ProbeRun, String> {
    let Proto::Compact { t_commit_rounds } = scenario.proto else {
        return Err("the persistence probe reads executed sequences: compact proto only".into());
    };
    let BlockSchedule::FreshPerRound { fraction } = scenario.schedule else {
        return Err(format!(
            "the persistence probe can only replay fresh_per_round schedules \
             (the others draw their masks in compile_blocking, which is crate-private): {:?}",
            scenario.schedule
        ));
    };
    if !scenario.manual_blocks.is_empty() {
        return Err(
            "manual_blocks union onto the mask AFTER the draw; the probe would replay a \
             different population — drop them or extend the probe"
                .into(),
        );
    }
    scenario.validate()?;

    let mut state = SmrState::new_with_certs(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &scenario.injections,
        scenario.traffic.as_deref().unwrap_or(&[]),
        scenario.client_model,
        scenario.partition.as_deref(),
        false,
        scenario.merge_policy,
        scenario.repeated_commit,
    );
    let blocked = state.blocked_count(fraction);

    let mut probe = ProbeRun {
        report: state.report(),
        rounds: 0,
        truncated: false,
        latch_round: None,
        r_v: None,
        transience: None,
        split_after_rv: Vec::new(),
        retractions_after_rv: Vec::new(),
        heals: None,
        shrink_events: 0,
        exec_len_rv: None,
        observations: Vec::new(),
        expected_safety: opts.expected_safety,
        replay_ok: None,
    };
    let mut obs_rounds: Vec<usize> = Vec::new();
    let mut lengths = vec![0usize; scenario.n];

    for round in 1..=scenario.max_rounds {
        let retractions_before = state.retraction_counts().0;
        let status = state.step_sampled(blocked);
        let retracted = state.retraction_counts().0 - retractions_before;
        probe.rounds = round;
        // Borrowed, never materialised: a copy here puts the whole executed history in the per-round cost.
        let seqs: Vec<&ExecSeq> = (0..scenario.n)
            .map(|i| {
                state
                    .executed_entries(i)
                    .expect("the probe runs the compact or recovery rule")
            })
            .collect();

        for (prev, seq) in lengths.iter_mut().zip(&seqs) {
            if seq.len() < *prev {
                probe.shrink_events += 1;
            }
            *prev = seq.len();
        }
        if probe.latch_round.is_none() && !state.safety_ok() {
            probe.latch_round = Some(round);
        }

        let split = !prefixes_consistent(&seqs);
        if split && probe.r_v.is_none() {
            probe.r_v = Some(round);
            probe.exec_len_rv = seqs.iter().map(|s| s.len()).max();
            obs_rounds = OBS_MULTIPLES
                .iter()
                .map(|m| round + m * t_commit_rounds as usize)
                .collect();
        }
        if probe.r_v.is_some() {
            probe.split_after_rv.push(split);
            probe.retractions_after_rv.push(retracted);
        }

        let last_round = probe.r_v.is_some_and(|r_v| {
            opts.stop_rounds_after_violation
                .is_some_and(|cap| round == r_v + cap)
        }) || round == scenario.max_rounds
            || status.dead.is_some();
        if split && (obs_rounds.contains(&round) || last_round) {
            {
                let owned: Vec<Vec<Entry>> = seqs.iter().map(|s| s.to_vec()).collect();
                let flat: Vec<&[Entry]> = owned.iter().map(Vec::as_slice).collect();
                probe
                    .observations
                    .push(observe(round, &flat, probe.exec_len_rv.unwrap_or(0)));
            }
        }
        if last_round {
            probe.truncated = round < scenario.max_rounds;
            break;
        }
    }

    probe.report = state.report();
    probe.transience = probe.r_v.map(|r_v| transience(r_v, &probe.split_after_rv));
    probe.heals = probe
        .r_v
        .map(|_| heals(&probe.split_after_rv, &probe.retractions_after_rv));
    probe.replay_ok = opts
        .expected_safety
        .map(|expected| expected == probe.report.safety_ok);
    Ok(probe)
}

pub const PROBE_CSV_HEADER: &str = "exp,n,k,ell,beta,sigma,rate,t_commit,horizon,client_model,seed,\
terminal,rounds,truncated,commands,complete,safety,latch_round,r_v,latch_agrees,expected_safety,\
replay_ok,r_h,heal_rounds,episodes,incons_rounds,phi,shrink_events,exec_len_rv,obs_n,d_rv,d_1t,\
d_last,obs_last_round,delta_d,common_last,branches_rv,branch1_rv,branch2_rv,off_majority_nodes_rv,\
first_mismatch_rv,last_mismatch_rv,displaced_after_rv_last,dup_exec_max,sn_regress_max,\
fork_branch_rv,d_fork_rv,d_fork_1t,d_fork_last,delta_d_fork,retractions,retraction_rewrites,\
first_retraction_round,heals,heals_by_retraction,r_h_retractions";

fn rate(scenario: &SmrScenario) -> f64 {
    scenario
        .traffic
        .as_deref()
        .unwrap_or(&[])
        .first()
        .map_or(0.0, |phase| {
            phase
                .arrivals_pmf
                .iter()
                .enumerate()
                .map(|(i, w)| i as f64 * w)
                .sum()
        })
}

fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

/// One CSV row per probe run; persistence columns of a never-split run are empty, not zero.
/// The retraction columns are filled in every run: Def 1.5 does not wait for a split.
pub fn probe_row(exp: &str, scenario: &SmrScenario, probe: &ProbeRun) -> String {
    let beta = match scenario.schedule {
        BlockSchedule::FreshPerRound { fraction } => fraction,
        _ => f64::NAN,
    };
    let t_commit = match scenario.proto {
        Proto::Compact { t_commit_rounds } => t_commit_rounds,
        _ => 0,
    };
    let terminal = match probe.report.terminal {
        SmrTerminal::Ran { .. } => "ran",
        SmrTerminal::Dead { .. } => "dead",
        SmrTerminal::Halted { .. } => "halted",
        SmrTerminal::Failed { .. } => {
            panic!("persistence probe cannot score a failed partial report")
        }
    };
    let complete = probe
        .report
        .commands
        .iter()
        .filter(|c| c.status == CommandStatus::Complete)
        .count();
    let client_model = match scenario.client_model {
        ClientModel::Unique => "unique".to_string(),
        ClientModel::Pool { clients } => format!("pool{clients}"),
    };

    let t = probe.transience.as_ref();
    let at = |m: usize| {
        probe
            .r_v
            .map(|r_v| r_v + m * t_commit as usize)
            .and_then(|round| probe.observations.iter().find(|o| o.round == round))
    };
    let first = probe.observations.first();
    let last = probe.observations.last();
    let delta_d = match (at(1), last) {
        (Some(base), Some(last)) => Some(last.d as i64 - base.d as i64),
        _ => None,
    };
    // The registered pair is A against the largest branch that disagrees with it (`d_fork`).
    let delta_d_fork = match (at(1).and_then(|o| o.d_fork), last.and_then(|o| o.d_fork)) {
        (Some(base), Some(last)) => Some(last as i64 - base as i64),
        _ => None,
    };

    format!(
        "{exp},{},{},{},{beta},{},{},{t_commit},{},{client_model},{},\
         {terminal},{},{},{},{complete},{},{},{},{},{},{},\
         {},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},\
         {},{},{},{},{},{}",
        scenario.n,
        scenario.cfg.k,
        scenario.cfg.ell,
        scenario.sigma,
        rate(scenario),
        scenario.max_rounds,
        scenario.seed,
        probe.rounds,
        probe.truncated,
        probe.report.commands.len(),
        probe.report.safety_ok,
        opt(probe.latch_round),
        opt(probe.r_v),
        opt(probe.r_v.map(|r_v| probe.latch_round == Some(r_v))),
        opt(probe.expected_safety),
        opt(probe.replay_ok),
        opt(t.and_then(|t| t.r_h)),
        opt(t.and_then(|t| t.heal_rounds)),
        opt(t.map(|t| t.episodes)),
        opt(t.map(|t| t.inconsistent_rounds)),
        opt(t.map(|t| format!("{:.4}", t.phi))),
        opt(probe.r_v.map(|_| probe.shrink_events)),
        opt(probe.exec_len_rv),
        opt(probe.r_v.map(|_| probe.observations.len())),
        opt(first.map(|o| o.d)),
        opt(at(1).map(|o| o.d)),
        opt(last.map(|o| o.d)),
        opt(last.map(|o| o.round)),
        opt(delta_d),
        opt(last.map(|o| o.common)),
        opt(first.map(|o| o.branches)),
        opt(first.map(|o| o.branch1)),
        opt(first.map(|o| o.branch2)),
        opt(first.map(|o| o.inconsistent_nodes)),
        opt(first.and_then(|o| o.first_mismatch)),
        opt(first.and_then(|o| o.last_mismatch)),
        opt(last.map(|o| o.displaced_after_rv)),
        opt(probe.observations.iter().map(|o| o.dup_exec).max()),
        opt(probe.observations.iter().map(|o| o.sn_regress).max()),
        opt(first.and_then(|o| o.fork_branch)),
        opt(first.and_then(|o| o.d_fork)),
        opt(at(1).and_then(|o| o.d_fork)),
        opt(last.and_then(|o| o.d_fork)),
        opt(delta_d_fork),
        probe.report.retractions,
        probe.report.retraction_rewrites,
        opt(probe.report.first_retraction_round),
        opt(probe.heals.as_ref().map(|h| h.heals)),
        opt(probe.heals.as_ref().map(|h| h.by_retraction)),
        opt(probe.heals.as_ref().and_then(|h| h.r_h_retractions)),
    )
}

pub fn observe(round: usize, seqs: &[&[Entry]], exec_len_rv: usize) -> Observation {
    let branches = partition_branches(seqs);
    let a = seqs[branches[0][0]];
    let b = branches.get(1).map_or(a, |members| seqs[members[0]]);
    let disp = displacement(a, b);
    let overlap = a.len().min(b.len());
    let mismatches: Vec<usize> = (0..overlap).filter(|&p| a[p] != b[p]).collect();
    // `None` is legitimate: the split test uses the longest sequence, this the largest branch.
    let fork = forked_branch(seqs);

    Observation {
        round,
        branches: branches.len(),
        inconsistent_nodes: nodes_off_the_majority_chain(seqs, &branches),
        branch1: branches[0].len(),
        branch2: branches.get(1).map_or(0, Vec::len),
        exec_len_a: a.len(),
        exec_len_b: b.len(),
        common: disp.common,
        d: disp.d,
        fork_branch: fork.map(|f| {
            branches
                .iter()
                .find(|members| members[0] == f)
                .map_or(0, Vec::len)
        }),
        d_fork: fork.map(|f| displacement(a, seqs[f]).d),
        d_mutual: mutual_fork_pair(seqs).map(|(x, y)| displacement(seqs[x], seqs[y]).d),
        first_mismatch: mismatches.first().copied(),
        last_mismatch: mismatches.last().copied(),
        displaced_after_rv: disp
            .displaced_idx
            .iter()
            .filter(|&&i| i >= exec_len_rv)
            .count(),
        dup_exec: max_duplicate_executions(seqs),
        sn_regress: max_sn_regressions(seqs),
    }
}

/// Nodes whose sequence is not a prefix of the anchor: the branch most nodes' sequences are prefixes of,
/// the longer then the smaller sequence on a tie, so the count does not depend on node order.
fn nodes_off_the_majority_chain(seqs: &[&[Entry]], branches: &[Vec<usize>]) -> usize {
    let reps: Vec<(&[Entry], usize)> = branches.iter().map(|m| (seqs[m[0]], m.len())).collect();
    let support = |anchor: &[Entry]| {
        reps.iter()
            .filter(|(rep, _)| anchor.starts_with(rep))
            .map(|&(_, members)| members)
            .sum::<usize>()
    };
    let (anchor, _) = reps
        .iter()
        .max_by(|(a, _), (b, _)| {
            support(a)
                .cmp(&support(b))
                .then(a.len().cmp(&b.len()))
                .then_with(|| b.cmp(a))
        })
        .expect("non-empty");
    seqs.iter().filter(|s| !anchor.starts_with(s)).count()
}

pub fn partition_branches(seqs: &[&[Entry]]) -> Vec<Vec<usize>> {
    let mut reps: Vec<(&[Entry], Vec<usize>)> = Vec::new();
    for (i, seq) in seqs.iter().enumerate() {
        match reps.iter_mut().find(|(rep, _)| rep == seq) {
            Some((_, members)) => members.push(i),
            None => reps.push((seq, vec![i])),
        }
    }
    let mut branches: Vec<Vec<usize>> = reps.into_iter().map(|(_, m)| m).collect();
    branches.sort_by_key(|m| (std::cmp::Reverse(m.len()), m[0]));
    branches
}

pub fn duplicate_executions(seq: &[Entry]) -> usize {
    let mut seen: HashSet<(u32, u64)> = HashSet::new();
    seq.iter()
        .filter_map(Entry::client_sn)
        .filter(|key| !seen.insert(*key))
        .count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Displacement {
    pub common: usize,
    pub lcs: usize,
    pub d: usize,
    pub displaced_idx: Vec<usize>,
}

/// D(a, b) over shared commands; a repeated command counts at its first execution only.
pub fn displacement(a: &[Entry], b: &[Entry]) -> Displacement {
    let mut pos_in_b: HashMap<u64, usize> = HashMap::new();
    for (i, e) in b.iter().enumerate() {
        if let Entry::Cmd(c) = e {
            pos_in_b.entry(c.op).or_insert(i);
        }
    }
    let mut seen: HashSet<u64> = HashSet::new();
    let common: Vec<(usize, usize)> = a
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            Entry::Cmd(c) if seen.insert(c.op) => pos_in_b.get(&c.op).map(|&p| (i, p)),
            _ => None,
        })
        .collect();

    let keep = longest_increasing(&common.iter().map(|&(_, p)| p).collect::<Vec<_>>());
    let mut kept = vec![false; common.len()];
    for &i in &keep {
        kept[i] = true;
    }
    let displaced_idx: Vec<usize> = common
        .iter()
        .zip(&kept)
        .filter(|&(_, &k)| !k)
        .map(|(&(i, _), _)| i)
        .collect();

    Displacement {
        common: common.len(),
        lcs: keep.len(),
        d: common.len() - keep.len(),
        displaced_idx,
    }
}

fn longest_increasing(values: &[usize]) -> Vec<usize> {
    let mut tails: Vec<usize> = Vec::new();
    let mut parent: Vec<Option<usize>> = vec![None; values.len()];
    for (i, &v) in values.iter().enumerate() {
        let pile = tails.partition_point(|&t| values[t] < v);
        parent[i] = pile.checked_sub(1).map(|p| tails[p]);
        if pile == tails.len() {
            tails.push(i);
        } else {
            tails[pile] = i;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        out.push(i);
        cur = parent[i];
    }
    out.reverse();
    out
}
