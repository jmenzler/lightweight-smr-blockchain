//! The per-node footprint answers "what would one real server hold": every structure a node
//! references is charged to it in full, as if it owned a private copy, where the shared
//! accounting charges one allocation once. It is an observer, so watching must not change the run.

use protocol::compact::{Entry, Timed};
use sim::smr::{
    ClientModel, NodeBytes, Proto, SmrScenario, SmrState, TrafficPhase, footprint, point_mass_pmf,
    run_smr, run_smr_observed,
};
use sim::{BlockSchedule, Config};

fn cell(proto: Proto) -> SmrScenario {
    SmrScenario {
        n: 32,
        seed: 975_300,
        cfg: Config::default(),
        sigma: 1.0,
        proto,
        injections: vec![],
        max_rounds: 120,
        schedule: BlockSchedule::FreshPerRound { fraction: 0.1 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(1),
        }]),
        client_model: ClientModel::Unique,
        certs: true,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

fn recovery() -> Proto {
    Proto::Recovery {
        t_window_rounds: 20,
        resend_until_acked: false,
        prefix_mismatch: sim::smr::PrefixMismatch::Abort,
    }
}

fn node(log: u64) -> Option<NodeBytes> {
    Some(NodeBytes {
        log,
        ..Default::default()
    })
}

#[test]
fn footprint_reads_median_and_max_over_the_non_bot_nodes() {
    let f = footprint(&[node(10), None, node(30), node(20)]).expect("three nodes hold a log");
    assert_eq!(f.counted, 3);
    assert_eq!(f.median, 20, "the upper median of 10, 20, 30");
    assert_eq!(f.max, 30);
    assert_eq!(
        f.median_node,
        node(20).unwrap(),
        "the breakdown is the median node's"
    );
    assert_eq!(
        footprint(&[node(10), node(20)]).unwrap().median,
        20,
        "even count: upper median"
    );
    assert!(
        footprint(&[None, None]).is_none(),
        "every node ⊥: no footprint"
    );
}

#[test]
fn node_total_is_the_sum_of_its_categories() {
    let b = NodeBytes {
        log: 1,
        executed: 2,
        sn: 4,
        checkpoint: 8,
        certs: 16,
    };
    assert_eq!(b.total(), 31);
}

#[test]
fn every_recovery_node_is_charged_in_full() {
    let s = cell(recovery());
    let mut checked = 0;
    run_smr_observed(&s, &mut |state: &SmrState| {
        let per = state.node_bytes().expect("recovery nodes are walkable");
        assert_eq!(per.len(), s.n);
        if state.round() < s.max_rounds {
            return;
        }
        let acc = state.memory_accounting().expect("recovery accounting");
        let mut log_entries = 0u64;
        for (i, b) in per.iter().enumerate() {
            let nd = state.recovery_node(i).expect("a recovery node");
            let Some(b) = b else {
                assert!(
                    nd.log_seq().is_none(),
                    "node {i} holds a log but was left out"
                );
                continue;
            };
            let log = nd.log_seq().expect("a counted node holds a log");
            log_entries += (log.len() * size_of::<Timed>()) as u64;
            assert!(b.log >= (log.len() * size_of::<Timed>()) as u64);
            assert_eq!(
                b.executed,
                nd.shared_state().logical_len() * size_of::<Entry>() as u64,
                "a full run forgets nothing: node {i} holds its whole executed sequence"
            );
            // the sn maps in map units: (u32 client, u64 sn) per entry, at 1.5×
            let map = |len: usize| ((len * 12) as u64 * 3) / 2;
            assert_eq!(b.sn, map(nd.shared_state().sn_len()), "node {i}: sn map");
            let cp = nd.checkpoint();
            let pre = cp.p.as_ref().map_or(0, Vec::len);
            assert!(
                pre > 0 || cp.s.retained_len() > 0,
                "node {i}: a checkpoint after six window ends"
            );
            assert_eq!(
                b.checkpoint,
                (cp.s.retained_len() * size_of::<Entry>()) as u64
                    + map(cp.s.sn_len())
                    + (pre * size_of::<Timed>()) as u64,
                "node {i}: checkpoint = S's executed entries + its sn map + |P| entries"
            );
            assert!(b.certs > 0, "node {i}: the cert-on run stores §5 windows");
            checked += 1;
        }
        assert!(
            log_entries >= acc.log_len_bytes,
            "per-node logs {log_entries} B undercut the shared occupancy {} B",
            acc.log_len_bytes
        );
    });
    assert!(checked > 0, "some node held a log at the end");
}

#[test]
fn compact_nodes_are_walked_too() {
    let s = cell(Proto::Compact {
        t_commit_rounds: 20,
    });
    let mut last = None;
    run_smr_observed(&s, &mut |state: &SmrState| last = state.node_bytes());
    let per = last.expect("compact nodes are walkable");
    let f = footprint(&per).expect("some compact node holds a log");
    assert!(
        f.median_node.executed > 0,
        "compact executed commands by round 120"
    );
    assert_eq!(f.median_node.checkpoint, 0, "compact has no checkpoint");
    assert_eq!(
        f.median_node.certs, 0,
        "compact §5 state lives in the cert layer, not the node"
    );
}

#[test]
fn walking_every_round_leaves_the_run_byte_identical() {
    for proto in [
        recovery(),
        Proto::Compact {
            t_commit_rounds: 20,
        },
    ] {
        let s = cell(proto);
        let plain = run_smr(&s);
        let observed = run_smr_observed(&s, &mut |state: &SmrState| {
            state.node_bytes();
            state.simulator_node_bytes();
        });
        assert_eq!(plain, observed, "observing the footprint changed the run");
    }
}

/// The simulator charges each shared allocation once, so it holds no more than the servers
/// would hold apart: the sharing factor n × median ÷ simulator bytes rests on this.
#[test]
fn the_simulator_holds_no_more_than_the_servers_apart() {
    for proto in [
        recovery(),
        Proto::Compact {
            t_commit_rounds: 20,
        },
    ] {
        let s = cell(proto);
        let mut last = None;
        run_smr_observed(&s, &mut |state: &SmrState| {
            last = Some((
                state.node_bytes().expect("walkable"),
                state.simulator_node_bytes().expect("walkable"),
                state.standalone_bytes().expect("walkable"),
            ));
            let (_, shared, all) = last.as_ref().unwrap();
            assert!(shared <= all, "round {}: {shared} > {all}", state.round());
        });
        let (per, shared, all) = last.expect("observed");
        let held: u64 = per.iter().flatten().map(NodeBytes::total).sum();
        assert!(shared > 0);
        assert!(
            all >= held,
            "every server's bytes include the ones holding a log"
        );
        // same population (all n servers) and the same units on both sides: sharing only removes
        assert!(
            shared <= all,
            "{shared} B shared against {all} B apart: the walks count different units"
        );
        assert!(shared < all, "the simulator shares nothing?");
    }
}
