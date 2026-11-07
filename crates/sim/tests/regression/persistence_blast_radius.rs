//! inconsistent_nodes counts the servers off the majority branch, independent of node order
//! and of which branch executed furthest.

use protocol::compact::{ClientCommand, Entry};
use sim::persistence::observe;

fn seq(ops: &[u64]) -> Vec<Entry> {
    ops.iter()
        .map(|&op| {
            Entry::Cmd(ClientCommand {
                client: 1000 + op as u32,
                sn: 1,
                op,
            })
        })
        .collect()
}

#[test]
fn a_longer_minority_branch_does_not_make_the_majority_inconsistent() {
    let majority = seq(&[1, 2]);
    let deviant = seq(&[1, 3, 4]);
    let mut seqs: Vec<&[Entry]> = vec![&majority; 5];
    seqs.push(&deviant);
    let obs = observe(10, &seqs, 0);
    assert_eq!((obs.branch1, obs.inconsistent_nodes), (5, 1));
}

#[test]
fn inconsistent_nodes_is_invariant_under_node_order() {
    let majority = seq(&[1, 2]);
    let deviant = seq(&[1, 3]);
    let mut deviant_last: Vec<&[Entry]> = vec![&majority; 5];
    deviant_last.push(&deviant);
    let mut deviant_first: Vec<&[Entry]> = vec![&deviant];
    deviant_first.extend(vec![&majority[..]; 5]);
    assert_eq!(
        observe(10, &deviant_last, 0).inconsistent_nodes,
        observe(10, &deviant_first, 0).inconsistent_nodes
    );
}

#[test]
fn prefixes_of_the_majority_branch_are_consistent_with_it() {
    let majority = seq(&[1, 2, 3]);
    let behind = seq(&[1]);
    let deviant = seq(&[1, 9]);
    let seqs: Vec<&[Entry]> = vec![&majority, &majority, &behind, &deviant];
    assert_eq!(observe(10, &seqs, 0).inconsistent_nodes, 1);
}

#[test]
fn an_even_split_is_broken_by_content_not_by_node_order() {
    let a = seq(&[1, 2]);
    let b = seq(&[1, 3, 4]);
    let forward: Vec<&[Entry]> = vec![&a, &b];
    let backward: Vec<&[Entry]> = vec![&b, &a];
    let (f, r) = (
        observe(10, &forward, 0).inconsistent_nodes,
        observe(10, &backward, 0).inconsistent_nodes,
    );
    assert_eq!((f, r), (1, 1));
}

#[test]
fn a_fully_consistent_fleet_has_no_inconsistent_nodes() {
    let long = seq(&[1, 2, 3]);
    let short = seq(&[1, 2]);
    let seqs: Vec<&[Entry]> = vec![&long, &short, &long];
    assert_eq!(observe(10, &seqs, 0).inconsistent_nodes, 0);
}

#[test]
fn the_probe_column_carries_the_new_name() {
    let header: Vec<&str> = sim::persistence::PROBE_CSV_HEADER.split(',').collect();
    assert!(header.contains(&"off_majority_nodes_rv"));
    assert!(!header.contains(&"incons_nodes_rv"));
}
