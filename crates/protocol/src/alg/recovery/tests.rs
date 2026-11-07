use super::*;

fn cmd(client: u32, sn: u64) -> ClientCommand {
    ClientCommand { client, sn, op: sn }
}

/// S_i holds sn(1) = 2 while C_i.S still holds the genesis sn map.
fn node_whose_s_i_runs_ahead_of_c_i() -> RecoveryNode {
    let mut node = RecoveryNode::new(Config::default(), 8, false);
    node.s_i = Arc::new(SharedState::from_entries(Vec::new(), [(1, 2)].into()));
    node
}

// The Alg 5 box (p. 26): sn(c) is "assumed to be included in S_i", so the client rule reads S_i, not C_i.S.
#[test]
fn the_client_rule_reads_sn_c_from_s_i() {
    let node = node_whose_s_i_runs_ahead_of_c_i();
    assert_eq!(node.client_triage(&cmd(1, 3)), Triage::Amplify);
    assert_eq!(node.client_triage(&cmd(1, 2)), Triage::AckCommitted);
    assert_eq!(node.client_triage(&cmd(1, 1)), Triage::Ignore);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "S_i and C_i.S disagree on sn(c)")]
fn a_client_command_checks_that_s_i_and_c_i_agree_on_sn_c() {
    node_whose_s_i_runs_ahead_of_c_i().on_client_command(&cmd(1, 3));
}
