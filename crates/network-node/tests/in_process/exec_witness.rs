//! The compact executed-prefix witness tracks S_i through Alg 5's S_i := S_j adoption.

use network_node::collect::aggregate_smr;
use network_node::engine::RoundEngine;
use network_node::record::RoundRecord;
use network_node::spec::NodeRunSpec;
use network_node::wire::{Msg, ReplyPayload};
use protocol::compact::{ClientCommand, Entry, Timed};
use protocol::shared_state::SharedState;
use std::collections::BTreeMap;

type Reply = Option<(Vec<Timed>, Option<SharedState>)>;

fn engine(id: u32) -> RoundEngine {
    let spec = NodeRunSpec::from_json(
        r#"{"scenario":{"smr":{"n":4,"seed":3,"k":6,"ell":3,"sigma":1.0,
            "proto":{"kind":"compact","t_commit_rounds":1},"injections":[],
            "max_rounds":8,"schedule":{"kind":"fresh_per_round","fraction":0.0}}},
            "sync":"timer"}"#,
    )
    .expect("spec");
    RoundEngine::new(&spec, id).expect("engine")
}

fn agg_spec(rounds: usize) -> NodeRunSpec {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"smr":{{"n":2,"seed":1,"k":6,"ell":3,"sigma":1.0,
            "proto":{{"kind":"compact","t_commit_rounds":1}},"injections":[],
            "max_rounds":{rounds}}}}},"sync":"timer"}}"#
    ))
    .expect("spec")
}

fn cmd(sn: u64, op: u64) -> Entry {
    Entry::Cmd(ClientCommand { client: 1, sn, op })
}

fn timed(entry: Entry, round: u64) -> Timed {
    Timed { entry, round }
}

fn state_of(entries: Vec<Entry>, client_sn: u64) -> SharedState {
    SharedState::from_entries(entries, BTreeMap::from([(1u32, client_sn)]))
}

/// One engine round: ℓ = 3 identical crafted replies, or none when `replies` is None.
fn run_round(e: &mut RoundEngine, round: u64, replies: Reply) -> RoundRecord {
    e.on_round_start();
    if let Some((log, state)) = replies {
        for slot in 0..3u16 {
            e.on_message(Msg::PullReply {
                round,
                from: 1,
                slot,
                payload: ReplyPayload::Compact {
                    log: log.clone(),
                    state: state.clone(),
                },
            });
        }
    }
    e.on_round_end()
}

fn quiet(round: u64) -> Reply {
    Some((vec![timed(Entry::Nop(round), round - 1)], None))
}

fn commit_now(entry: Entry, round: u64) -> Reply {
    Some((
        vec![timed(Entry::Nop(round), round - 1), timed(entry, round - 1)],
        None,
    ))
}

/// The ⊥ node receives a state reply carrying `adopted`, which it takes as S_i.
fn adopt(adopted: SharedState, round: u64) -> Reply {
    Some((vec![timed(Entry::Nop(round), round - 1)], Some(adopted)))
}

fn last_hash_at(recs: &[RoundRecord], len: usize) -> Option<u64> {
    recs.iter()
        .flat_map(|r| r.exec_prefix.iter())
        .filter(|(l, _)| *l == len)
        .map(|(_, h)| *h)
        .next_back()
}

// Alg 5, p. 26: S_i := S_j, so a later commit extends the adopted sequence.
#[test]
fn adoption_to_an_equal_length_divergent_state_is_visible_to_the_safety_gate() {
    let (mut a, mut b) = (engine(0), engine(1));
    let (mut ra, mut rb) = (Vec::new(), Vec::new());
    ra.push(run_round(&mut a, 1, commit_now(cmd(1, 100), 1)));
    rb.push(run_round(&mut b, 1, commit_now(cmd(1, 100), 1)));
    ra.push(run_round(&mut a, 2, None));
    rb.push(run_round(&mut b, 2, quiet(2)));
    ra.push(run_round(
        &mut a,
        3,
        adopt(state_of(vec![cmd(1, 200)], 1), 3),
    ));
    rb.push(run_round(&mut b, 3, quiet(3)));
    ra.push(run_round(&mut a, 4, commit_now(cmd(2, 300), 4)));
    rb.push(run_round(&mut b, 4, commit_now(cmd(2, 300), 4)));

    // A executed [200, 300]; B executed [100, 300].
    let (ha, hb) = (last_hash_at(&ra, 2), last_hash_at(&rb, 2));
    assert!(
        ha.is_some() && hb.is_some(),
        "both report length 2: {ha:?} {hb:?}"
    );
    assert_ne!(
        ha, hb,
        "[200, 300] and [100, 300] must not share a witness hash"
    );
    let out = aggregate_smr(&[ra, rb], &agg_spec(4), None).expect("aggregate");
    assert!(!out.gates.safety_ok, "the fork must trip the safety gate");
}

// No fork (pp. 29-30): adopting a shorter state and recommitting forks from the peer.
#[test]
fn adoption_of_a_shorter_state_then_recommit_is_visible_to_the_safety_gate() {
    let (mut a, mut b) = (engine(0), engine(1));
    let (mut ra, mut rb) = (Vec::new(), Vec::new());
    for (round, op) in [(1, 100), (2, 200)] {
        ra.push(run_round(&mut a, round, commit_now(cmd(round, op), round)));
        rb.push(run_round(&mut b, round, commit_now(cmd(round, op), round)));
    }
    ra.push(run_round(&mut a, 3, None));
    rb.push(run_round(&mut b, 3, quiet(3)));
    ra.push(run_round(
        &mut a,
        4,
        adopt(state_of(vec![cmd(1, 100)], 1), 4),
    ));
    rb.push(run_round(&mut b, 4, quiet(4)));
    ra.push(run_round(&mut a, 5, commit_now(cmd(2, 300), 5)));
    rb.push(run_round(&mut b, 5, commit_now(cmd(2, 300), 5)));

    // A executed [100, 300]; B executed [100, 200, 300].
    let out = aggregate_smr(&[ra, rb], &agg_spec(5), None).expect("aggregate");
    assert!(
        !out.gates.safety_ok,
        "[100, 300] and [100, 200, 300] differ at position 2; the gate reported safe"
    );
}

// No fork (pp. 29-30): adopting a state equal to the peer's own keeps the gate clean.
#[test]
fn adoption_of_an_agreeing_state_keeps_the_gate_clean() {
    let (mut a, mut b) = (engine(0), engine(1));
    let (mut ra, mut rb) = (Vec::new(), Vec::new());
    for (round, op) in [(1, 100), (2, 200)] {
        ra.push(run_round(&mut a, round, commit_now(cmd(round, op), round)));
        rb.push(run_round(&mut b, round, commit_now(cmd(round, op), round)));
    }
    ra.push(run_round(&mut a, 3, None));
    rb.push(run_round(&mut b, 3, quiet(3)));
    ra.push(run_round(
        &mut a,
        4,
        adopt(state_of(vec![cmd(1, 100)], 1), 4),
    ));
    rb.push(run_round(&mut b, 4, quiet(4)));
    ra.push(run_round(&mut a, 5, commit_now(cmd(2, 200), 5)));
    rb.push(run_round(&mut b, 5, quiet(5)));

    let out = aggregate_smr(&[ra, rb], &agg_spec(5), None).expect("aggregate");
    assert!(out.gates.safety_ok, "both executed [100, 200]");
}

fn tracked_engine(id: u32) -> RoundEngine {
    let spec = NodeRunSpec::from_json(
        r#"{"scenario":{"smr":{"n":4,"seed":3,"k":6,"ell":3,"sigma":1.0,
            "proto":{"kind":"compact","t_commit_rounds":5},"injections":[],
            "max_rounds":8,"schedule":{"kind":"fresh_per_round","fraction":0.0}}},
            "sync":"timer","tracked_ops":[100]}"#,
    )
    .expect("spec");
    let mut e = RoundEngine::new(&spec, id).expect("engine");
    for round in 1..=5 {
        run_round(&mut e, round, None);
    }
    e
}

/// A reply log holding x_d first and the op behind it, `age` rounds old.
fn behind_nop(op_entry: Entry, round: u64, age: u64) -> Reply {
    Some((
        vec![
            timed(Entry::Nop(round), round - age),
            timed(op_entry, round - age),
        ],
        None,
    ))
}

// A tracked command has one logical position (among executed commands) whether or not a server
// has committed it yet, so the position and prefix digests cannot depend on the commit stage.
#[test]
fn a_tracked_command_has_the_same_position_and_prefix_before_and_after_commit() {
    let (mut committed, mut pending) = (tracked_engine(0), tracked_engine(1));
    let fresh = run_round(&mut pending, 6, behind_nop(cmd(1, 100), 6, 1));
    let old = run_round(&mut committed, 6, behind_nop(cmd(1, 100), 6, 5));
    let (c, p) = (&old.tracked[0], &fresh.tracked[0]);
    assert!(c.present && p.present);
    assert_eq!(c.pos, p.pos, "x_d in front of the command is not a command");
    assert_eq!(c.prefix_hash, p.prefix_hash);
}

// T_B counts a command only while it sits in logs, as the simulator's coverage latch does.
#[test]
fn a_committed_command_is_no_longer_in_the_log() {
    let (mut committed, mut pending) = (tracked_engine(0), tracked_engine(1));
    let fresh = run_round(&mut pending, 6, behind_nop(cmd(1, 100), 6, 1));
    let old = run_round(&mut committed, 6, behind_nop(cmd(1, 100), 6, 5));
    assert!(fresh.tracked[0].in_log);
    assert!(!old.tracked[0].in_log, "committed entries leave the log");
}
