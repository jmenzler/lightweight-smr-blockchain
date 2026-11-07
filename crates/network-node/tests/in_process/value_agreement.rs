//! Gossip extinction and spread must not look alike in the collector's summary.

use network_node::collect::aggregate;
use network_node::harness;
use network_node::record::PostState;
use network_node::spec::NodeRunSpec;

fn gossip_spec(seed: u64, holders: usize) -> NodeRunSpec {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"gossip":{{"n":16,"seed":{seed},"k":6,"ell":3,"holders":{holders},
            "x":7,"x0":0,"max_rounds":30}}}},"sync":"timer"}}"#
    ))
    .expect("spec")
}

fn finals(spec: &NodeRunSpec) -> Vec<Option<u64>> {
    harness::run(spec)
        .iter()
        .map(|r| match r.last().expect("rounds").post {
            PostState::Value(v) => v,
            _ => panic!("value mode"),
        })
        .collect()
}

#[test]
fn a_lost_x_is_reported_as_agreement_on_x0_without_convergence() {
    let spec = gossip_spec(11, 1);
    assert!(
        finals(&spec).iter().all(|v| *v == Some(0)),
        "precondition: x goes extinct for this seed"
    );
    let out = aggregate(&harness::run(&spec), &spec).expect("aggregate");
    assert_eq!(out.summary.agreed_value, Some(0));
    assert_eq!(out.summary.convergence_round, None);
    assert_eq!(out.summary.target_holders.last(), Some(&0));
}

#[test]
fn a_spread_x_is_reported_as_convergence_on_x() {
    let spec = gossip_spec(3, 8);
    assert!(
        finals(&spec).iter().all(|v| *v == Some(7)),
        "precondition: x reaches every server for this seed"
    );
    let out = aggregate(&harness::run(&spec), &spec).expect("aggregate");
    assert_eq!(out.summary.agreed_value, Some(7));
    assert!(out.summary.convergence_round.is_some());
    assert_eq!(out.summary.target_holders.last(), Some(&16));
}
