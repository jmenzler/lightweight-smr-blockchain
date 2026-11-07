//! The SMR spec's `repeated_commit` policy must reach the node's compact core.

use network_node::engine::RoundEngine;
use network_node::record::PostState;
use network_node::spec::NodeRunSpec;
use network_node::wire::{Msg, ReplyPayload};
use protocol::compact::{ClientCommand, Entry, Timed};

fn spec(proto: &str, policy: &str) -> Result<NodeRunSpec, String> {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"smr":{{"n":4,"seed":3,"k":6,"ell":3,"sigma":1.0,
            "proto":{proto},"injections":[],"max_rounds":8,{policy}
            "schedule":{{"kind":"fresh_per_round","fraction":0.0}}}}}},"sync":"timer"}}"#
    ))
}

/// Executed length after the same aged command is served in two consecutive rounds.
fn executed_after_repeat(policy: &str) -> usize {
    let spec = spec(r#"{"kind":"compact","t_commit_rounds":1}"#, policy).expect("spec");
    let mut e = RoundEngine::new(&spec, 0).expect("engine");
    let cmd = Entry::Cmd(ClientCommand {
        client: 1,
        sn: 1,
        op: 100,
    });
    let mut exec_len = 0;
    for round in 1..=2u64 {
        e.on_round_start();
        for slot in 0..3u16 {
            e.on_message(Msg::PullReply {
                round,
                from: 1,
                slot,
                payload: ReplyPayload::Compact {
                    log: vec![
                        Timed {
                            entry: Entry::Nop(round),
                            round: round - 1,
                        },
                        Timed {
                            entry: cmd.clone(),
                            round: round - 1,
                        },
                    ],
                    state: None,
                },
            });
        }
        match e.on_round_end().post {
            PostState::Log { exec_len: l, .. } => exec_len = l,
            other => panic!("log mode expected, got {other:?}"),
        }
    }
    exec_len
}

#[test]
fn execute_policy_runs_a_repeated_commit_twice() {
    assert_eq!(executed_after_repeat(""), 2);
}

#[test]
fn skip_policy_runs_a_repeated_commit_once() {
    assert_eq!(
        executed_after_repeat(r#""repeated_commit":"skip","#),
        1,
        "the node must run the execute-once policy the spec names"
    );
}

#[test]
fn skip_policy_is_still_rejected_for_extended() {
    assert!(spec(r#"{"kind":"extended"}"#, r#""repeated_commit":"skip","#).is_err());
}

fn gap_reply(round: u64, slot: u16) -> Msg {
    Msg::PullReply {
        round,
        from: 1,
        slot,
        payload: ReplyPayload::Compact {
            log: vec![
                Timed {
                    entry: Entry::Nop(round),
                    round: round - 1,
                },
                Timed {
                    entry: Entry::Cmd(ClientCommand {
                        client: 7,
                        sn: 2,
                        op: 100,
                    }),
                    round: round - 1,
                },
            ],
            state: None,
        },
    }
}

#[test]
fn skip_policy_flags_an_sn_gap_in_the_record_and_keeps_the_node_running() {
    let spec = spec(
        r#"{"kind":"compact","t_commit_rounds":1}"#,
        r#""repeated_commit":"skip","#,
    )
    .expect("spec");
    let mut e = RoundEngine::new(&spec, 0).expect("engine");
    let mut flags = Vec::new();
    for round in 1..=3u64 {
        e.on_round_start();
        for slot in 0..3u16 {
            e.on_message(gap_reply(round, slot));
        }
        flags.push(e.on_round_end().sn_order_violation);
    }
    assert_eq!(
        flags,
        [true, true, true],
        "the refusal is latched and the frozen node is never stepped again"
    );
}

#[test]
fn execute_policy_never_flags_an_sn_gap() {
    let spec = spec(r#"{"kind":"compact","t_commit_rounds":1}"#, "").expect("spec");
    let mut e = RoundEngine::new(&spec, 0).expect("engine");
    e.on_round_start();
    for slot in 0..3u16 {
        e.on_message(gap_reply(1, slot));
    }
    assert!(!e.on_round_end().sn_order_violation);
}
