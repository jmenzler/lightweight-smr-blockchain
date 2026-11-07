//! RoundEngine SMR arms (extended Alg 3, compact Alg 5) — checklist §4
//! extended/compact rows. Full commit-cycle behavior (resend → AckCommitted)
//! lives in the acceptance suite's harness loop; these pin the per-message
//! semantics.

use network_node::engine::RoundEngine;
use network_node::record::PostState;
use network_node::spec::NodeRunSpec;
use network_node::wire::{AckKind, Msg, ReplyPayload};
use protocol::compact::{Entry, Timed};

fn smr_json(n: usize, proto: &str, tracked: &str) -> String {
    format!(
        r#"{{"scenario":{{"smr":{{"n":{n},"seed":3,"k":6,"ell":3,"sigma":1.0,
             "proto":{proto},"injections":[],"max_rounds":40,
             "schedule":{{"kind":"fresh_per_round","fraction":0.0}}}}}},
             "sync":"timer"{tracked}}}"#
    )
}

fn engine(n: usize, proto: &str, tracked: &str, id: u32) -> RoundEngine {
    let spec = NodeRunSpec::from_json(&smr_json(n, proto, tracked)).expect("spec");
    RoundEngine::new(&spec, id).expect("engine")
}

const EXT: &str = r#"{"kind":"extended"}"#;
const COMP: &str = r#"{"kind":"compact","t_commit_rounds":3}"#;

#[test]
fn extended_delivery_acks_and_amplifies() {
    let mut e = engine(4, EXT, "", 0);
    e.on_round_start();
    let out = e.on_message(Msg::ClientCmd {
        round: 1,
        client: 1,
        sn: 1,
        op: 500,
    });
    let acks: Vec<&Msg> = out
        .iter()
        .map(|(_, m)| m)
        .filter(|m| matches!(m, Msg::ClientAck { .. }))
        .collect();
    assert_eq!(acks.len(), 1);
    assert!(
        matches!(
            acks[0],
            Msg::ClientAck {
                kind: AckKind::Delivered,
                sn: 1,
                ..
            }
        ),
        "extended: a log-holding server delivers (the delivery signal is not specified by §3)"
    );
    // ⌈1.0·log₂ 4⌉ = 2 amplification pushes, self-inclusive draws
    let apps: Vec<&(u32, Msg)> = out
        .iter()
        .filter(|(_, m)| matches!(m, Msg::Append { .. }))
        .collect();
    assert_eq!(apps.len(), 2, "amp_count(1.0, 4)");
    for (_, m) in &apps {
        assert!(matches!(
            m,
            Msg::Append {
                op: 500,
                round: 1,
                ..
            }
        ));
    }
}

/// ℓ crafted log replies: keeps the extended log alive through the step
/// (< ℓ would correctly ⊥ the node and discard its appends — Alg 3 step 4).
fn feed_logs(e: &mut RoundEngine, round: u64, log: &[u64]) {
    for slot in 0..3u16 {
        e.on_message(Msg::PullReply {
            round,
            from: 1,
            slot,
            payload: ReplyPayload::Log(log.to_vec()),
        });
    }
}

#[test]
fn extended_duplicate_delivery_does_not_amplify_again() {
    let mut e = engine(4, EXT, "", 0);
    e.on_round_start();
    // land the op via delivery + append, with the log kept alive
    let out = e.on_message(Msg::ClientCmd {
        round: 1,
        client: 1,
        sn: 1,
        op: 500,
    });
    assert!(out.iter().any(|(_, m)| matches!(m, Msg::Append { .. })));
    e.on_message(Msg::Append {
        round: 1,
        from: 0,
        client: 1,
        sn: 1,
        op: 500,
    });
    feed_logs(&mut e, 1, &[sim::smr::SEED_COMMAND]);
    let rec = e.on_round_end();
    assert!(
        matches!(
            rec.post,
            PostState::Log {
                log_len: Some(2),
                ..
            }
        ),
        "log = [seed, 500] after append: {:?}",
        rec.post
    );

    e.on_round_start();
    let out2 = e.on_message(Msg::ClientCmd {
        round: 2,
        client: 1,
        sn: 1,
        op: 500,
    });
    assert!(
        out2.iter().any(|(_, m)| matches!(
            m,
            Msg::ClientAck {
                kind: AckKind::Delivered,
                ..
            }
        )),
        "delivery acks regardless"
    );
    assert!(
        !out2.iter().any(|(_, m)| matches!(m, Msg::Append { .. })),
        "op already in log: wants_amplify gates re-amplification"
    );
}

#[test]
fn extended_bottom_server_delivers_and_amplifies() {
    let mut e = engine(4, EXT, "", 0);
    e.on_round_start();
    e.on_round_end(); // zero replies < ℓ → log ⊥
    e.on_round_start();
    let out = e.on_message(Msg::ClientCmd {
        round: 2,
        client: 1,
        sn: 1,
        op: 500,
    });

    assert_eq!(
        out.iter()
            .filter(|(_, msg)| matches!(
                msg,
                Msg::ClientAck {
                    kind: AckKind::Delivered,
                    ..
                }
            ))
            .count(),
        1
    );
    assert_eq!(
        out.iter()
            .filter(|(_, msg)| matches!(msg, Msg::Append { op: 500, .. }))
            .count(),
        2
    );

    let record = e.on_round_end();
    assert!(!record.snap_held);
    assert_eq!(record.cli_recv, 1);
    assert_eq!(record.acks, 1);
    assert_eq!(record.app_sent, 2);
}

fn client_cmd(e: &mut RoundEngine, round: u64, client: u32, op: u64) -> Vec<(u32, Msg)> {
    e.on_message(Msg::ClientCmd {
        round,
        client,
        sn: 1,
        op,
    })
}

fn has_ack(out: &[(u32, Msg)], expected: AckKind) -> bool {
    out.iter()
        .any(|(_, m)| matches!(m, Msg::ClientAck { kind, .. } if *kind == expected))
}

fn append_count(out: &[(u32, Msg)]) -> usize {
    out.iter()
        .filter(|(_, m)| matches!(m, Msg::Append { .. }))
        .count()
}

/// ℓ = 3 compact replies carrying only the seed log, so the node stays non-⊥ through the step.
fn feed_seed_replies(e: &mut RoundEngine, round: u64) {
    for slot in 0..3u16 {
        e.on_message(Msg::PullReply {
            round,
            from: 1,
            slot,
            payload: ReplyPayload::Compact {
                log: vec![Timed {
                    entry: Entry::Nop(0),
                    round: 0,
                }],
                state: None,
            },
        });
    }
}

#[test]
fn compact_unseen_command_with_the_next_sn_is_amplified() {
    let mut e = engine(4, COMP, "", 0);
    e.on_round_start();
    let out = client_cmd(&mut e, 1, 1, 500);
    assert!(
        has_ack(&out, AckKind::Amplified),
        "L_i ≠ ⊥, sn == committed+1 and unseen → Amplify triage"
    );
    assert_eq!(append_count(&out), 2, "compact amplifies on Amplify triage");
}

#[test]
fn compact_resend_of_a_command_in_the_log_is_ignored_but_a_fresh_one_is_amplified() {
    // Alg 5: "L_i ≠ ⊥, sn(x) = sn(c)+1 and x ∉ L_i" amplifies; a command already in L_i falls
    // through to "otherwise, i ignores x". The node is non-⊥ (ℓ replies fed), so the Ignore
    // can only come from x ∈ L_i.
    let mut e = engine(4, COMP, "", 0);
    e.on_round_start();
    e.on_message(Msg::Append {
        round: 1,
        from: 3,
        client: 1,
        sn: 1,
        op: 500,
    });
    feed_seed_replies(&mut e, 1);
    let rec = e.on_round_end();
    assert!(
        matches!(
            rec.post,
            PostState::Log {
                log_len: Some(l),
                ..
            } if l >= 2
        ),
        "the node holds the appended command: {:?}",
        rec.post
    );

    e.on_round_start();
    let resend = client_cmd(&mut e, 2, 1, 500);
    assert!(has_ack(&resend, AckKind::Ignored), "x ∈ L_i → Ignored");
    assert_eq!(append_count(&resend), 0, "Ignore never re-amplifies");

    let fresh = client_cmd(&mut e, 2, 9, 900);
    assert!(
        has_ack(&fresh, AckKind::Amplified),
        "x ∉ L_i at the same non-⊥ server → Amplified"
    );
    assert_eq!(append_count(&fresh), 2);
}

#[test]
fn compact_bottom_server_ignores_every_command() {
    // Alg 5 triage needs L_i ≠ ⊥: a server that received < ℓ replies ignores the command.
    let mut e = engine(4, COMP, "", 0);
    e.on_round_start();
    let rec = e.on_round_end();
    assert!(matches!(rec.post, PostState::Log { log_len: None, .. }));
    e.on_round_start();
    let out = client_cmd(&mut e, 2, 1, 500);
    assert!(has_ack(&out, AckKind::Ignored));
    assert_eq!(append_count(&out), 0);
}

#[test]
fn compact_bot_requester_sets_needs_state_and_server_attaches_state() {
    // Engine A goes ⊥ (zero replies in round 1), so its round-2 pulls must
    // carry needs_state — and a healthy server's reply carries state.
    let mut a = engine(4, COMP, "", 0);
    a.on_round_start();
    a.on_round_end();
    let pulls = a.on_round_start();
    assert!(!pulls.is_empty());
    for (_, m) in &pulls {
        assert!(
            matches!(
                m,
                Msg::PullRequest {
                    needs_state: true,
                    ..
                }
            ),
            "⊥ requester asks for state (recovery bit b_i = 1)"
        );
    }

    let mut b = engine(4, COMP, "", 1);
    b.on_round_start();
    let (_, req) = pulls.into_iter().find(|(to, _)| *to == 1).unwrap_or({
        // no draw hit node 1 — craft the same request shape
        (
            1,
            Msg::PullRequest {
                round: 2,
                from: 0,
                slot: 0,
                needs_state: true,
            },
        )
    });
    // b is in round 1; a is pulling for round 2 — queue + drain would apply;
    // craft a current-round request instead to isolate the serving rule.
    let req = match req {
        Msg::PullRequest { slot, .. } => Msg::PullRequest {
            round: 1,
            from: 0,
            slot,
            needs_state: true,
        },
        _ => unreachable!(),
    };
    let out = b.on_message(req);
    assert_eq!(out.len(), 1);
    match &out[0].1 {
        Msg::PullReply {
            payload: ReplyPayload::Compact { state, .. },
            ..
        } => {
            assert!(state.is_some(), "needs_state request → state attached");
        }
        other => panic!("expected compact reply, got {other:?}"),
    }
}

#[test]
fn compact_healthy_requester_gets_no_state() {
    let mut a = engine(4, COMP, "", 0);
    let pulls = a.on_round_start();
    for (_, m) in &pulls {
        assert!(matches!(
            m,
            Msg::PullRequest {
                needs_state: false,
                ..
            }
        ));
    }
    let mut b = engine(4, COMP, "", 1);
    b.on_round_start();
    let out = b.on_message(Msg::PullRequest {
        round: 1,
        from: 0,
        slot: 0,
        needs_state: false,
    });
    match &out[0].1 {
        Msg::PullReply {
            payload: ReplyPayload::Compact { state, .. },
            ..
        } => {
            assert!(state.is_none(), "healthy requester: no state clone");
        }
        other => panic!("expected compact reply, got {other:?}"),
    }
}

#[test]
fn tracked_op_digests_report_presence_and_position() {
    let tracked = r#","tracked_ops":[500]"#;
    let mut e = engine(4, EXT, tracked, 0);
    e.on_round_start();
    feed_logs(&mut e, 1, &[sim::smr::SEED_COMMAND]);
    let rec = e.on_round_end();
    assert_eq!(rec.tracked.len(), 1);
    assert!(!rec.tracked[0].present, "op not yet anywhere");

    e.on_round_start();
    e.on_message(Msg::Append {
        round: 2,
        from: 3,
        client: 1,
        sn: 1,
        op: 500,
    });
    feed_logs(&mut e, 2, &[sim::smr::SEED_COMMAND]);
    let rec = e.on_round_end();
    assert!(rec.tracked[0].present, "landed via append");
    assert_eq!(rec.tracked[0].pos, Some(1), "0-based: after the seed");
    assert!(rec.tracked[0].prefix_hash.is_some());
}

#[test]
fn a_non_blocked_bottom_server_answers_no_pull() {
    for proto in [EXT, COMP] {
        let mut e = engine(4, proto, "", 0);
        e.on_round_start();
        e.on_round_end(); // no replies: fewer than ℓ, so the log is ⊥
        e.on_round_start();
        let out = e.on_message(Msg::PullRequest {
            round: 2,
            from: 1,
            slot: 0,
            needs_state: false,
        });
        assert!(out.is_empty(), "{proto}: ⊥ servers do not reply (Alg 3/5)");
        assert_eq!(e.on_round_end().rep_sent, 0);
    }
}

#[test]
fn a_blocked_smr_server_drops_commands_appends_and_pulls_and_ends_bottom() {
    for proto in [EXT, COMP] {
        let json = smr_json(4, proto, "").replace(
            r#"{"kind":"fresh_per_round","fraction":0.0}"#,
            r#"{"kind":"permanent","fraction":1.0}"#,
        );
        let spec = NodeRunSpec::from_json(&json).expect("spec");
        let mut e = RoundEngine::new(&spec, 0).expect("engine");
        assert!(
            e.on_round_start().is_empty(),
            "{proto}: blocked sends nothing"
        );
        let mut out = e.on_message(Msg::ClientCmd {
            round: 1,
            client: 1,
            sn: 1,
            op: 500,
        });
        out.extend(e.on_message(Msg::Append {
            round: 1,
            from: 1,
            client: 1,
            sn: 1,
            op: 500,
        }));
        out.extend(e.on_message(Msg::PullRequest {
            round: 1,
            from: 1,
            slot: 0,
            needs_state: false,
        }));
        assert!(out.is_empty(), "{proto}: nothing leaves a blocked server");
        let rec = e.on_round_end();
        assert_eq!((rec.cli_recv, rec.app_recv, rec.rep_sent), (0, 0, 0));
        assert_eq!(
            rec.app_blocked, 1,
            "the append is the blocking loss the model allows"
        );
        assert_eq!(
            rec.cli_blocked, 1,
            "a client command at a blocked server is counted, not silently dropped"
        );
        assert!(matches!(rec.post, PostState::Log { log_len: None, .. }));
    }
}
