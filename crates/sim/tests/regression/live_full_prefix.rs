//! The live-only T_E observer for compact and recovery (Lemma 3.8, p. 24, over executed part ++
//! log) against a brute-force oracle every round, and inert: switching it on changes nothing
//! but `prefix_fixed_round`.

use protocol::compact::{Entry, Timed};
use sim::Config;
use sim::smr::{
    ClientModel, PrefixMismatch, Proto, SmrReport, SmrState, TrafficPhase, UnsettledAge,
};
use std::collections::BTreeMap;

const N: usize = 48;

fn rec(resend_until_acked: bool) -> Proto {
    Proto::Recovery {
        t_window_rounds: 24,
        resend_until_acked,
        prefix_mismatch: PrefixMismatch::Abort,
    }
}

fn state(proto: Proto, observe: bool) -> SmrState {
    let mut s = SmrState::new(
        N,
        Config::default(),
        proto,
        1.0,
        11,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: vec![0.3, 0.4, 0.3],
        }],
        ClientModel::Unique,
        None,
    );
    if observe {
        s.observe_full_prefix();
    }
    s
}

fn step(s: &mut SmrState) {
    s.draw_arrivals();
    s.step_fraction(0.08);
}

fn logs(s: &SmrState) -> Vec<Option<Vec<Timed>>> {
    if s.recovery_node(0).is_some() {
        (0..N)
            .map(|i| s.recovery_node(i).unwrap().log_entries())
            .collect()
    } else {
        s.compact_logs()
    }
}

struct Oracle {
    lcp: usize,
    longest: usize,
    reference: Vec<Entry>,
    age: UnsettledAge,
    lagging: bool,
}

fn oracle(s: &SmrState) -> Option<Oracle> {
    let executed = s.executed_seqs();
    let logs = logs(s);
    let seqs: Vec<(usize, Vec<Entry>, &[Timed])> = (0..N)
        .filter_map(|i| {
            let log = logs[i].as_deref()?;
            let mut full = executed[i].clone();
            full.extend(log.iter().map(|t| t.entry.clone()));
            Some((executed[i].len(), full, log))
        })
        .collect();
    // the reference: longest executed part, then longest sequence, then lowest id
    let r = &seqs[(0..seqs.len())
        .rev()
        .max_by_key(|&i| (seqs[i].0, seqs[i].1.len()))?]
    .1;
    let agree: Vec<(usize, usize)> = seqs
        .iter()
        .map(|(_, full, _)| {
            (
                r.iter().zip(full).take_while(|(a, b)| a == b).count(),
                full.len(),
            )
        })
        .collect();
    let lcp = agree.iter().map(|&(a, _)| a).min().unwrap();
    // a sequence that agrees to its own end lags, it contradicts nobody
    let contested = agree
        .iter()
        .filter(|&&(a, len)| a < len)
        .map(|&(a, _)| a)
        .min()
        .unwrap_or(r.len())
        .min(r.len());
    let round = s.round() as u64;
    let age_past = |prefix: usize| {
        seqs.iter()
            .flat_map(|(ex, _, log)| log.iter().skip(prefix.saturating_sub(*ex)))
            .map(|t| round.saturating_sub(t.round))
            .max()
            .unwrap_or(0)
    };
    let longest = seqs.iter().map(|(_, full, _)| full.len()).max().unwrap();
    Some(Oracle {
        lcp,
        longest,
        reference: r[..lcp].to_vec(),
        age: UnsettledAge {
            full: age_past(lcp),
            contested: age_past(contested),
        },
        lagging: contested > lcp,
    })
}

/// Returns the rounds in which some server only lagged behind the reference.
fn check(proto: Proto, rounds: usize) -> usize {
    let mut s = state(proto, true);
    let mut first_fixed: BTreeMap<u64, usize> = BTreeMap::new();
    let mut contested_rounds = 0;
    let mut lagging_rounds = 0;
    for round in 1..=rounds {
        step(&mut s);
        assert!(
            s.safety_ok(),
            "{proto:?}: the scenario must stay prefix consistent"
        );
        let o = oracle(&s).expect("some server holds a log");
        if o.lcp < o.longest {
            contested_rounds += 1;
        }
        if o.lagging {
            lagging_rounds += 1;
        }
        assert_eq!(
            s.unsettled_ages()[round - 1],
            Some(o.age),
            "{proto:?} r{round}: unsettled age"
        );
        for e in &o.reference {
            if let Entry::Cmd(c) = e {
                first_fixed.entry(c.op).or_insert(round);
            }
        }
    }
    assert_eq!(s.unsettled_ages().len(), rounds);
    assert!(
        contested_rounds > rounds / 2,
        "{proto:?}: the logs must actually disagree somewhere"
    );
    let report: SmrReport = s.report();
    let mut ops: Vec<u64> = report.commands.iter().map(|c| c.op).collect();
    ops.sort_unstable();
    ops.dedup();
    assert_eq!(ops.len(), report.commands.len(), "ops identify commands");
    let mut fixed = 0;
    for c in &report.commands {
        assert_eq!(
            c.prefix_fixed_round,
            first_fixed.get(&c.op).copied(),
            "{proto:?} op {}",
            c.op
        );
        if let Some(f) = c.prefix_fixed_round {
            fixed += 1;
            assert!(
                c.delivered_round.is_some_and(|d| d <= f),
                "{proto:?} op {}: fixed before delivered",
                c.op
            );
        }
    }
    let committed = report
        .commands
        .iter()
        .filter(|c| c.committed_round.or(c.executed_round).is_some())
        .count();
    assert!(
        committed > 50 && fixed >= committed,
        "{proto:?}: {fixed} fixed, {committed} committed"
    );
    lagging_rounds
}

#[test]
fn compact_prefix_fixed_matches_the_brute_force_oracle() {
    check(
        Proto::Compact {
            t_commit_rounds: 24,
        },
        220,
    );
}

#[test]
fn recovery_prefix_fixed_matches_the_brute_force_oracle() {
    for resend_until_acked in [false, true] {
        let lagging = check(rec(resend_until_acked), 220);
        assert!(
            lagging > 0,
            "servers rejoining at a boundary lag behind for a round or more"
        );
    }
}

#[test]
fn the_observer_changes_nothing_but_prefix_fixed_round() {
    for proto in [
        Proto::Compact {
            t_commit_rounds: 24,
        },
        rec(true),
    ] {
        let (mut on, mut off) = (state(proto, true), state(proto, false));
        for _ in 0..150 {
            step(&mut on);
            step(&mut off);
        }
        assert!(off.unsettled_ages().is_empty(), "off records nothing");
        let mut seen = on.report();
        let plain = off.report();
        assert!(
            plain
                .commands
                .iter()
                .all(|c| c.prefix_fixed_round.is_none())
        );
        assert!(seen.commands.iter().any(|c| c.prefix_fixed_round.is_some()));
        for c in &mut seen.commands {
            c.prefix_fixed_round = None;
        }
        assert_eq!(
            serde_json::to_string(&seen).unwrap(),
            serde_json::to_string(&plain).unwrap(),
            "{proto:?}"
        );
    }
}
