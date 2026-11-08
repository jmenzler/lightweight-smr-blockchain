//! Extraction pins: hand-stepped `SmrState` must replay the batch runner
//! byte-identically for every schedule path (the Alg-1 `sim_state.rs`
//! pattern applied to SMR).

use sim::smr::{ClientModel, Injection, Proto, SmrScenario, SmrState, run_smr};
use sim::{BlockSchedule, BlockTarget, BlockWindow, Config};

fn scenario(proto: Proto, schedule: BlockSchedule) -> SmrScenario {
    SmrScenario {
        n: 48,
        seed: 71,
        cfg: Config::default(),
        sigma: 2.0,
        proto,
        injections: vec![
            Injection {
                round: 2,
                client: 1,
                op: 7,
                target: None,
            },
            Injection {
                round: 4,
                client: 2,
                op: 9,
                target: None,
            },
        ],
        max_rounds: 40,
        schedule,
        traffic: None,
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

fn hand_stepped(scenario: &SmrScenario) -> sim::smr::SmrReport {
    let mut state = SmrState::new(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &scenario.injections,
        scenario.traffic.as_deref().unwrap_or(&[]),
        ClientModel::Unique,
        None,
    );
    match &scenario.schedule {
        BlockSchedule::FreshPerRound { fraction } => {
            for _ in 0..scenario.max_rounds {
                let status = state.step_fraction(*fraction);
                if status.dead.is_some() {
                    break;
                }
            }
        }
        BlockSchedule::Windows(windows) => {
            for round in 1..=scenario.max_rounds {
                let mut mask = vec![false; scenario.n];
                for w in windows {
                    if (w.start_round..w.start_round + w.rounds).contains(&round) {
                        match &w.target {
                            BlockTarget::Nodes(ids) => {
                                for &id in ids {
                                    mask[id as usize] = true;
                                }
                            }
                            BlockTarget::SampleFraction(_) => unreachable!("test uses Nodes"),
                        }
                    }
                }
                let status = state.step_masked(&mask);
                if status.dead.is_some() {
                    break;
                }
            }
        }
        BlockSchedule::PerRoundSticky {
            background,
            targets,
        } => {
            let mut sticky = vec![false; scenario.n];
            for round in 1..=scenario.max_rounds {
                let count = state.blocked_count(targets.get(round - 1).copied().unwrap_or(0.0));
                state.adjust_sticky(&mut sticky, count);
                let bg = state.blocked_count(*background);
                let status = if bg > 0 {
                    let mut union = state.sample_mask(bg);
                    for (u, &s) in union.iter_mut().zip(&sticky) {
                        *u |= s;
                    }
                    state.step_masked(&union)
                } else {
                    state.step_masked(&sticky)
                };
                if status.dead.is_some() {
                    break;
                }
            }
        }
        other => unreachable!("test does not cover {other:?}"),
    }
    state.report()
}

#[test]
fn step_status_lists_holders_for_commands_still_spreading() {
    // The live node-grid view needs per-node holdings while a command
    // spreads; once it is in all non-⊥ logs the list is dropped (every
    // non-⊥ NodeGlance holds it by definition).
    let s = scenario(
        Proto::Extended,
        BlockSchedule::FreshPerRound { fraction: 0.0 },
    );
    let mut state = SmrState::new(
        s.n,
        s.cfg,
        s.proto,
        s.sigma,
        s.seed,
        &s.injections,
        &[],
        ClientModel::Unique,
        None,
    );
    let s1 = state.step_fraction(0.0);
    assert!(s1.spreading.is_empty(), "nothing injected before round 2");
    let s2 = state.step_fraction(0.0);
    let sp = s2
        .spreading
        .iter()
        .find(|c| c.client == 1)
        .expect("op 7 spreading after its injection round");
    assert!(!sp.holders.is_empty(), "amp receivers hold it post-step");
    assert!(sp.holders.len() < s.n, "not yet saturated");
    let mut done = None;
    for _ in 0..20 {
        let st = state.step_fraction(0.0);
        if !st.spreading.iter().any(|c| c.client == 1) {
            done = Some(st);
            break;
        }
    }
    let report = state.report();
    assert!(
        report.commands[0].all_logs_round.is_some(),
        "spreading entry disappears exactly when the command covers all logs"
    );
    assert!(done.is_some());
}

/// One client with two commands spreading at once (a pipelined client here; a pool client
/// reused after its ack in general): each spreading entry names its own command, and its holder
/// set is that command's alone, never its sibling's.
#[test]
fn spreading_entries_are_keyed_by_command_not_by_client() {
    let mut state = SmrState::new(
        48,
        Config::default(),
        Proto::Compact { t_commit_rounds: 8 },
        2.0,
        71,
        &[
            Injection {
                round: 2,
                client: 1,
                op: 7,
                target: None,
            },
            Injection {
                round: 3,
                client: 1,
                op: 8,
                target: None,
            },
        ],
        &[],
        ClientModel::Unique,
        None,
    );
    let mut both = 0;
    for _ in 0..12 {
        let status = state.step_fraction(0.0);
        let logs = state.compact_logs();
        for s in &status.spreading {
            let holders: Vec<u32> = logs
                .iter()
                .enumerate()
                .filter(|(_, log)| {
                    log.as_ref().is_some_and(|l| {
                        l.iter().any(|t| {
                            matches!(t.entry, protocol::compact::Entry::Cmd(cc) if cc.op == s.op)
                        })
                    })
                })
                .map(|(i, _)| i as u32)
                .collect();
            assert_eq!(s.holders, holders, "round {}: op {}", status.round, s.op);
        }
        let ops: Vec<u64> = status.spreading.iter().map(|s| s.op).collect();
        if ops == [7, 8] {
            both += 1;
            assert!(status.spreading.iter().all(|s| s.client == 1));
        }
    }
    assert!(both > 0, "both commands of client 1 must spread at once");
}

#[test]
fn masked_stepping_replays_windows_batch() {
    let s = scenario(
        Proto::Extended,
        BlockSchedule::Windows(vec![BlockWindow {
            start_round: 3,
            rounds: 2,
            target: BlockTarget::Nodes(vec![0, 1, 2]),
        }]),
    );
    assert_eq!(hand_stepped(&s), run_smr(&s));
}

#[test]
fn sampled_stepping_replays_fresh_per_round_batch() {
    let s = scenario(
        Proto::Extended,
        BlockSchedule::FreshPerRound { fraction: 0.1 },
    );
    assert_eq!(hand_stepped(&s), run_smr(&s));
}

#[test]
fn sticky_with_background_replays_batch() {
    let s = scenario(
        Proto::Extended,
        BlockSchedule::PerRoundSticky {
            background: 0.05,
            targets: vec![0.2; 40],
        },
    );
    assert_eq!(hand_stepped(&s), run_smr(&s));
}

#[test]
fn compact_stepping_replays_batch_including_safety() {
    let s = scenario(
        Proto::Compact { t_commit_rounds: 6 },
        BlockSchedule::FreshPerRound { fraction: 0.1 },
    );
    let (stepped, batch) = (hand_stepped(&s), run_smr(&s));
    assert_eq!(stepped, batch);
    assert!(batch.safety_ok);
}
