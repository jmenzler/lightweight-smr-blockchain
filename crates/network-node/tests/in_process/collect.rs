//! Collector goldens: byte-exact sim-schema CSV, the persistent-unanimity
//! convergence rule, message aggregation under the F6 accounting rule, gates.

use network_node::collect::{ClientLog, aggregate, aggregate_smr};
use network_node::record::{PostState, RoundRecord, TrackedDigest};
use network_node::spec::NodeRunSpec;

fn spec_rounds(rounds: usize) -> NodeRunSpec {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"median":{{"n":2,"seed":1,"k":6,"ell":3,
            "init":{{"kind":"distinct"}},"max_rounds":{rounds}}}}},"sync":"timer"}}"#
    ))
    .expect("spec")
}

fn spec() -> NodeRunSpec {
    spec_rounds(4)
}

fn rec(r: u64, blocked: bool, snap_held: bool, post: Option<u64>) -> RoundRecord {
    RoundRecord {
        r,
        blocked,
        snap_held,
        post: PostState::Value(post),
        replies: 0,
        req_sent: if blocked { 0 } else { 6 },
        req_recv: if blocked { 0 } else { 6 },
        rep_sent: 2,
        rep_recv: 2,
        app_sent: 0,
        app_recv: 0,
        app_blocked: 0,
        cli_recv: 0,
        acks: 0,
        bytes_out: 100,
        bytes_in: 40,
        late_rep: 0,
        late_app: 0,
        dropped_past: 0,
        late_cli: 0,
        early_cli: 0,
        cli_blocked: 0,
        sn_order_violation: false,
        encode_err: 0,
        transport_err: 0,
        exec_prefix: Vec::new(),
        t_last_reply_us: 0,
        t_step_done_us: 0,
        t_step_compute_us: 0,
        tracked: Vec::new(),
    }
}

#[test]
fn summary_aggregates_round_timing_tails() {
    // 2 nodes × 2 rounds; nearest-rank p99 over 4 samples is the max sample.
    let mut recs = vec![
        vec![rec(1, false, true, Some(7)), rec(2, false, true, Some(7))],
        vec![rec(1, false, true, Some(7)), rec(2, false, true, Some(7))],
    ];
    let last_reply = [10u64, 20, 30, 40];
    let step_done = [1u64, 2, 3, 100];
    let step_compute = [5u64, 6, 7, 50];
    let mut i = 0;
    for node in recs.iter_mut() {
        for r in node.iter_mut() {
            r.t_last_reply_us = last_reply[i];
            r.t_step_done_us = step_done[i];
            r.t_step_compute_us = step_compute[i];
            i += 1;
        }
    }
    let out = aggregate(&recs, &spec()).expect("aggregate");
    assert_eq!(out.summary.timing.t_last_reply_us_max, 40);
    assert_eq!(out.summary.timing.t_last_reply_us_p99, 40);
    assert_eq!(out.summary.timing.t_step_done_us_max, 100);
    assert_eq!(out.summary.timing.t_step_done_us_p99, 100);
    assert_eq!(out.summary.timing.t_step_compute_us_max, 50);
    assert_eq!(out.summary.timing.t_step_compute_us_p99, 50);
}

#[test]
fn metrics_csv_is_byte_exact_sim_schema() {
    // A trace Alg 1 can produce: node 0 holds 7 throughout; node 1 holds 9 in r1, is blocked
    // (⊥) in r2 and adopts 7 from node 0's reply in r3.
    let records = vec![
        vec![
            rec(1, false, true, Some(7)),
            rec(2, false, true, Some(7)),
            rec(3, false, true, Some(7)),
            rec(4, false, true, Some(7)),
        ],
        vec![
            rec(1, false, true, Some(9)),
            rec(2, true, true, None),
            rec(3, false, false, Some(7)),
            rec(4, false, true, Some(7)),
        ],
    ];
    let out = aggregate(&records, &spec()).expect("aggregate");
    let want = "round,holders,undecided,blocked,useful,distinct_values\n\
                1,2,0,0,2,2\n\
                2,1,1,1,1,1\n\
                3,2,0,0,1,1\n\
                4,2,0,0,2,1\n";
    assert_eq!(out.metrics_csv, want);
    // r1 holds two values; from r2 the holders are unanimous on 7 and stay so.
    assert_eq!(out.summary.convergence_round, Some(2));
}

#[test]
fn never_unanimous_reports_none() {
    let records = vec![
        vec![rec(1, false, true, Some(1)), rec(2, false, true, Some(1))],
        vec![rec(1, false, true, Some(2)), rec(2, false, true, Some(2))],
    ];
    let out = aggregate(&records, &spec()).expect("aggregate");
    assert_eq!(out.summary.convergence_round, None);
}

#[test]
fn extinct_population_reports_none() {
    let records = vec![
        vec![rec(1, false, true, Some(1)), rec(2, false, false, None)],
        vec![rec(1, false, true, Some(1)), rec(2, false, false, None)],
    ];
    let out = aggregate(&records, &spec()).expect("aggregate");
    assert_eq!(
        out.summary.convergence_round, None,
        "all-bot horizon is extinction, not agreement"
    );
}

#[test]
fn messages_csv_sums_the_cluster_per_round() {
    let records = vec![
        vec![rec(1, false, true, Some(7))],
        vec![rec(1, true, true, None)],
    ];
    let out = aggregate(&records, &spec()).expect("aggregate");
    let want = "round,requests,replies,appends,client_msgs,acks,bytes_out,bytes_in\n\
                1,6,4,0,0,0,200,80\n";
    assert_eq!(
        out.messages_csv, want,
        "blocked node contributed 0 requests"
    );
}

#[test]
fn gates_surface_validity_violations_and_ragged_rounds() {
    let mut bad = rec(2, false, true, Some(1));
    bad.late_rep = 2;
    bad.dropped_past = 1;
    let records = vec![
        vec![rec(1, false, true, Some(1)), bad],
        vec![rec(1, false, true, Some(1))], // ragged: one round missing
    ];
    let out = aggregate(&records, &spec()).expect("aggregate");
    assert_eq!(out.gates.late_rep, 2);
    assert_eq!(out.gates.dropped_past, 1);
    assert!(!out.gates.rounds_consistent, "ragged per-node round counts");
    assert!(!out.gates.clean());
}

#[test]
fn empty_input_is_an_error_not_a_panic() {
    assert!(aggregate(&[], &spec()).is_err());
}

// --- SMR aggregation --------------------------------------------------------

fn smr_spec_rounds(rounds: usize) -> NodeRunSpec {
    NodeRunSpec::from_json(&format!(
        r#"{{"scenario":{{"smr":{{"n":2,"seed":1,"k":6,"ell":3,"sigma":1.0,
            "proto":{{"kind":"compact","t_commit_rounds":3}},"injections":[],
            "max_rounds":{rounds}}}}},"sync":"timer"}}"#
    ))
    .expect("spec")
}

fn smr_spec() -> NodeRunSpec {
    smr_spec_rounds(2)
}

fn smr_rec(r: u64, log_len: Option<usize>, exec_len: usize, log_hash: Option<u64>) -> RoundRecord {
    let mut rec = rec(r, false, log_len.is_some(), None);
    rec.post = PostState::Log {
        log_len,
        exec_len,
        log_hash,
    };
    rec
}

#[test]
fn smr_metrics_csv_matches_the_ten_column_schema() {
    let records = vec![
        vec![
            smr_rec(1, Some(3), 1, Some(0xAA)),
            smr_rec(2, Some(4), 2, Some(0xBB)),
        ],
        vec![
            smr_rec(1, Some(3), 1, Some(0xAA)),
            smr_rec(2, None, 1, None), // went ⊥
        ],
    ];
    let out = aggregate_smr(
        &records,
        &smr_spec(),
        Some(&ClientLog {
            arrivals: vec![2, 0],
            ..client_log(0)
        }),
    )
    .expect("aggregate");
    let want = "round,nonbot_logs,blocked,useful,distinct_logs,max_log_len,min_executed_len,max_executed_len,arrivals,lcp_len\n\
                1,2,0,2,1,3,1,1,2,1\n\
                2,1,0,1,1,4,1,2,0,1\n";
    assert_eq!(out.metrics_csv, want);
}

#[test]
fn extended_leaves_lcp_empty_never_a_computed_zero() {
    let spec = NodeRunSpec::from_json(
        r#"{"scenario":{"smr":{"n":2,"seed":1,"k":6,"ell":3,"sigma":1.0,
            "proto":{"kind":"extended"},"injections":[],"max_rounds":1}},
            "sync":"timer"}"#,
    )
    .expect("spec");
    let records = vec![vec![smr_rec(1, Some(3), 0, Some(1))]];
    let out = aggregate_smr(&records, &spec, None).expect("aggregate");
    assert!(
        out.metrics_csv.ends_with(
            ",0,
"
        ),
        "extended lcp cell must be empty: {}",
        out.metrics_csv
    );
}

#[test]
fn smr_arrivals_default_to_zero_without_client_data() {
    let records = vec![vec![smr_rec(1, Some(1), 0, Some(1))]];
    let out = aggregate_smr(&records, &smr_spec(), None).expect("aggregate");
    assert!(
        out.metrics_csv.ends_with("1,1,0,1,1,1,0,0,0,0\n"),
        "{}",
        out.metrics_csv
    );
}

#[test]
fn encode_errors_fail_the_gates() {
    let mut bad = rec(1, false, true, Some(1));
    bad.encode_err = 1;
    let records = vec![vec![bad], vec![rec(1, false, true, Some(1))]];
    let out = aggregate(&records, &spec()).expect("aggregate");
    assert_eq!(out.gates.encode_err, 1);
    assert!(
        !out.gates.clean(),
        "encode faults are config errors, never silence"
    );
}

// --- safety gate + SMR landmarks ------------------------------------------

fn smr_rec_full(
    r: u64,
    log_len: Option<usize>,
    exec_prefix: Vec<(usize, u64)>,
    tracked: Vec<TrackedDigest>,
) -> RoundRecord {
    let mut rec = rec(r, false, log_len.is_some(), None);
    let exec_len = exec_prefix.last().map_or(0, |(l, _)| *l);
    rec.post = PostState::Log {
        log_len,
        exec_len,
        log_hash: Some(1),
    };
    rec.exec_prefix = exec_prefix;
    rec.tracked = tracked;
    rec
}

fn digest(op: u64, present: bool, pos: Option<usize>, prefix_hash: Option<u64>) -> TrackedDigest {
    TrackedDigest {
        op,
        present,
        in_log: present,
        pos,
        prefix_hash,
    }
}

#[test]
fn divergent_executed_prefixes_fail_the_safety_gate() {
    // Both nodes execute two entries; they agree at length 1 and differ at 2.
    let a = vec![smr_rec_full(1, Some(1), vec![(1, 0xA1), (2, 0xB1)], vec![])];
    let b = vec![smr_rec_full(1, Some(1), vec![(1, 0xA1), (2, 0xB2)], vec![])];
    let out = aggregate_smr(&[a, b], &smr_spec(), None).expect("aggregate");
    assert!(
        !out.gates.safety_ok,
        "split brain must latch the safety gate"
    );
    assert!(!out.gates.clean());
}

#[test]
fn agreeing_prefixes_of_different_lengths_stay_safe() {
    // Node b is one entry behind — a prefix, not a divergence.
    let a = vec![smr_rec_full(1, Some(1), vec![(1, 0xA1), (2, 0xB1)], vec![])];
    let b = vec![smr_rec_full(1, Some(1), vec![(1, 0xA1)], vec![])];
    let out = aggregate_smr(&[a, b], &smr_spec_rounds(1), None).expect("aggregate");
    assert!(out.gates.safety_ok);
    assert!(out.gates.clean());
}

#[test]
fn landmarks_derive_from_tracked_digests() {
    // op 700: in every non-⊥ log from round 2 (T_B); positions/prefixes agree
    // only from round 3 (T_E).
    let a = vec![
        smr_rec_full(1, Some(1), vec![], vec![digest(700, false, None, None)]),
        smr_rec_full(
            2,
            Some(2),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF1))],
        ),
        smr_rec_full(
            3,
            Some(2),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF9))],
        ),
        smr_rec_full(
            4,
            Some(2),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF9))],
        ),
    ];
    let b = vec![
        smr_rec_full(1, Some(1), vec![], vec![digest(700, false, None, None)]),
        smr_rec_full(
            2,
            Some(2),
            vec![],
            vec![digest(700, true, Some(2), Some(0xF2))],
        ),
        smr_rec_full(
            3,
            Some(2),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF9))],
        ),
        smr_rec_full(
            4,
            Some(2),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF9))],
        ),
    ];
    let out = aggregate_smr(&[a, b], &smr_spec(), None).expect("aggregate");
    let lm = out
        .landmarks
        .iter()
        .find(|l| l.op == 700)
        .expect("tracked op landmarks");
    assert_eq!(
        lm.all_logs_round,
        Some(2),
        "T_B: present in every non-⊥ log"
    );
    assert_eq!(
        lm.prefix_fixed_round,
        Some(3),
        "T_E: position + prefix agreed and never changed after"
    );
}

#[test]
fn landmarks_stay_none_when_agreement_never_settles() {
    let a = vec![
        smr_rec_full(
            1,
            Some(1),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF1))],
        ),
        smr_rec_full(
            2,
            Some(1),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF1))],
        ),
    ];
    let b = vec![
        smr_rec_full(
            1,
            Some(1),
            vec![],
            vec![digest(700, true, Some(2), Some(0xF2))],
        ),
        smr_rec_full(
            2,
            Some(1),
            vec![],
            vec![digest(700, true, Some(2), Some(0xF2))],
        ),
    ];
    let out = aggregate_smr(&[a, b], &smr_spec(), None).expect("aggregate");
    let lm = out
        .landmarks
        .iter()
        .find(|l| l.op == 700)
        .expect("landmarks");
    assert_eq!(lm.all_logs_round, Some(1));
    assert_eq!(lm.prefix_fixed_round, None, "never agreed on a position");
}

#[test]
fn transient_agreement_resets_the_te_candidate() {
    // Rounds 1-2 agree only because the divergent node sits ⊥; it rejoins in
    // round 3 with a different prefix, so the round-1 candidate must not
    // survive — T_E is the round-4 re-agreement.
    let a = vec![
        smr_rec_full(
            1,
            Some(1),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF1))],
        ),
        smr_rec_full(
            2,
            Some(1),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF1))],
        ),
        smr_rec_full(
            3,
            Some(1),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF1))],
        ),
        smr_rec_full(
            4,
            Some(1),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF1))],
        ),
    ];
    let b = vec![
        smr_rec_full(1, None, vec![], vec![]),
        smr_rec_full(2, None, vec![], vec![]),
        smr_rec_full(
            3,
            Some(1),
            vec![],
            vec![digest(700, true, Some(2), Some(0xF2))],
        ),
        smr_rec_full(
            4,
            Some(1),
            vec![],
            vec![digest(700, true, Some(1), Some(0xF1))],
        ),
    ];
    let out = aggregate_smr(&[a, b], &smr_spec(), None).expect("aggregate");
    let lm = out
        .landmarks
        .iter()
        .find(|l| l.op == 700)
        .expect("landmarks");
    assert_eq!(
        lm.prefix_fixed_round,
        Some(4),
        "T_E restarts after the transient rounds-1..2 agreement breaks"
    );
}

// --- message reconciliation -------------------------------------------------

fn two_nodes() -> Vec<Vec<RoundRecord>> {
    vec![
        vec![rec(1, false, true, Some(1))],
        vec![rec(1, false, true, Some(1))],
    ]
}

#[test]
fn a_balanced_run_reconciles() {
    let out = aggregate(&two_nodes(), &spec_rounds(1)).expect("aggregate");
    assert!(out.gates.clean(), "{:?}", out.gates);
}

#[test]
fn replies_sent_but_never_received_fail_the_gates() {
    let mut records = two_nodes();
    records[1][0].rep_recv = 0;
    let out = aggregate(&records, &spec_rounds(1)).expect("aggregate");
    assert_eq!(out.gates.rep_unreconciled, 2);
    assert!(!out.gates.clean(), "2 replies vanished without a trace");
}

#[test]
fn replies_counted_late_still_reconcile_but_fail_through_late_rep() {
    let mut records = two_nodes();
    records[1][0].rep_recv = 0;
    records[1][0].late_rep = 2;
    let out = aggregate(&records, &spec_rounds(1)).expect("aggregate");
    assert_eq!(out.gates.rep_unreconciled, 0);
    assert!(!out.gates.clean());
}

#[test]
fn requests_sent_but_never_received_fail_the_gates() {
    let mut records = two_nodes();
    records[0][0].req_recv = 4;
    let out = aggregate(&records, &spec_rounds(1)).expect("aggregate");
    assert_eq!(out.gates.req_unreconciled, 2);
    assert!(!out.gates.clean());
}

#[test]
fn appends_lost_in_transit_fail_but_appends_to_blocked_servers_do_not() {
    let mut records = two_nodes();
    records[0][0].app_sent = 3;
    records[1][0].app_recv = 2;
    records[1][0].app_blocked = 1;
    let out = aggregate(&records, &spec_rounds(1)).expect("aggregate");
    assert_eq!(out.gates.app_unreconciled, 0);
    assert!(out.gates.clean(), "{:?}", out.gates);
    records[1][0].app_blocked = 0;
    let out = aggregate(&records, &spec_rounds(1)).expect("aggregate");
    assert_eq!(out.gates.app_unreconciled, 1);
    assert!(!out.gates.clean());
}

#[test]
fn transport_errors_fail_the_gates() {
    let mut records = two_nodes();
    records[0][0].transport_err = 1;
    let out = aggregate(&records, &spec_rounds(1)).expect("aggregate");
    assert_eq!(out.gates.transport_err, 1);
    assert!(!out.gates.clean());
}

#[test]
fn smr_aggregation_applies_the_same_reconciliation() {
    let mut a = smr_rec(1, Some(1), 0, Some(1));
    a.rep_sent = 3;
    let b = smr_rec(1, Some(1), 0, Some(1));
    let out = aggregate_smr(&[vec![a], vec![b]], &smr_spec(), None).expect("aggregate");
    assert_eq!(out.gates.rep_unreconciled, 1);
    assert!(!out.gates.clean());
}

// --- value modes: what the population agreed on ------------------------------

fn priority_spec() -> NodeRunSpec {
    NodeRunSpec::from_json(
        r#"{"scenario":{"priority":{"n":4,"seed":1,"k":6,"ell":3,
            "init":{"kind":"distinct"},"max_rounds":2}},"sync":"timer"}"#,
    )
    .expect("spec")
}

fn gossip_spec() -> NodeRunSpec {
    NodeRunSpec::from_json(
        r#"{"scenario":{"gossip":{"n":4,"seed":1,"k":6,"ell":3,"holders":1,
            "x":7,"x0":0,"max_rounds":2}},"sync":"timer"}"#,
    )
    .expect("spec")
}

fn all_hold(values: &[Option<u64>]) -> Vec<Vec<RoundRecord>> {
    values
        .iter()
        .map(|v| vec![rec(1, false, true, *v), rec(2, false, true, *v)])
        .collect()
}

#[test]
fn gossip_extinction_is_agreement_on_x0_not_convergence() {
    let out = aggregate(&all_hold(&[Some(0); 4]), &gossip_spec()).expect("aggregate");
    assert_eq!(out.summary.agreed_value, Some(0));
    assert_eq!(out.summary.agreed_round, Some(1));
    assert_eq!(out.summary.convergence_round, None, "x went extinct");
    assert_eq!(out.summary.target_holders, vec![0, 0]);
}

#[test]
fn gossip_full_spread_converges_on_x() {
    let out = aggregate(&all_hold(&[Some(7); 4]), &gossip_spec()).expect("aggregate");
    assert_eq!(out.summary.agreed_value, Some(7));
    assert_eq!(out.summary.convergence_round, Some(1));
    assert_eq!(out.summary.target_value, Some(7));
    assert_eq!(out.summary.target_holders, vec![4, 4]);
}

#[test]
fn priority_agreement_below_the_maximum_is_not_convergence() {
    // distinct init over n = 4: the maximum initial value is 3
    let low = aggregate(&all_hold(&[Some(1); 4]), &priority_spec()).expect("aggregate");
    assert_eq!(low.summary.target_value, Some(3));
    assert_eq!(low.summary.agreed_value, Some(1));
    assert_eq!(low.summary.convergence_round, None);
    let max = aggregate(&all_hold(&[Some(3); 4]), &priority_spec()).expect("aggregate");
    assert_eq!(max.summary.convergence_round, Some(1));
}

#[test]
fn target_holders_count_only_servers_holding_the_target() {
    let values = [Some(7), Some(0), Some(7), None];
    let out = aggregate(&all_hold(&values), &gossip_spec()).expect("aggregate");
    assert_eq!(out.summary.target_holders, vec![2, 2]);
    assert_eq!(out.summary.agreed_value, None);
}

#[test]
fn median_has_no_target_and_keeps_its_convergence_rule() {
    let out = aggregate(&all_hold(&[Some(5); 2]), &spec()).expect("aggregate");
    assert_eq!(out.summary.target_value, None);
    assert!(out.summary.target_holders.is_empty());
    assert_eq!(out.summary.agreed_value, Some(5));
    assert_eq!(out.summary.convergence_round, Some(1));
}

#[test]
fn summary_json_exports_the_agreed_value_and_target_holders() {
    let out = aggregate(&all_hold(&[Some(0); 4]), &gossip_spec()).expect("aggregate");
    let json: serde_json::Value =
        serde_json::from_str(&network_node::collect::summary_json(&out)).expect("valid json");
    assert_eq!(json["agreed_value"], 0);
    assert_eq!(json["agreed_round"], 1);
    assert!(json["convergence_round"].is_null());
    assert_eq!(json["target_value"], 7);
    assert_eq!(json["target_holders"], serde_json::json!([0, 0]));
}

#[test]
fn late_client_commands_fail_the_gates() {
    let mut records = two_nodes();
    records[0][0].late_cli = 2;
    let out = aggregate(&records, &spec()).expect("aggregate");
    assert_eq!(out.gates.late_cli, 2);
    assert!(!out.gates.clean());
}

// --- what the safety gate and the horizon gate check --------------------------

#[test]
fn a_held_value_nobody_started_with_fails_the_value_mode_gate() {
    // distinct init at n = 2 starts the nodes on 0 and 1
    let records = vec![
        vec![rec(1, false, true, Some(0))],
        vec![rec(1, false, true, Some(99))],
    ];
    let out = aggregate(&records, &spec_rounds(1)).expect("aggregate");
    assert!(!out.gates.safety_ok);
    assert!(
        out.gates.network_clean(),
        "a bad value is not a network fault"
    );
    assert!(!out.gates.clean());
}

#[test]
fn a_run_cut_short_of_the_horizon_fails_even_when_every_node_stopped_alike() {
    let records = vec![
        vec![rec(1, false, true, Some(1)), rec(2, false, true, Some(1))],
        vec![rec(1, false, true, Some(1)), rec(2, false, true, Some(1))],
    ];
    let out = aggregate(&records, &spec_rounds(4)).expect("aggregate");
    assert!(!out.gates.rounds_consistent, "2 of 4 rounds on every node");
    let out = aggregate(&records, &spec_rounds(2)).expect("aggregate");
    assert!(out.gates.rounds_consistent);
}

#[test]
fn a_safety_violation_leaves_the_network_gates_clean() {
    let a = vec![smr_rec_full(1, Some(1), vec![(1, 0xA1)], vec![])];
    let b = vec![smr_rec_full(1, Some(1), vec![(1, 0xA2)], vec![])];
    let out = aggregate_smr(&[a, b], &smr_spec_rounds(1), None).expect("aggregate");
    assert!(out.gates.network_clean());
    assert!(!out.gates.safety_ok);
    assert!(!out.gates.clean());
}

#[test]
fn a_command_committed_before_it_covered_every_log_never_latches_t_b() {
    // node a has executed op 700 and dropped it from its log while node b first receives it
    let held = |in_log: bool| TrackedDigest {
        op: 700,
        present: true,
        in_log,
        pos: Some(0),
        prefix_hash: Some(0xF0),
    };
    let a = vec![
        smr_rec_full(1, Some(1), vec![], vec![held(false)]),
        smr_rec_full(2, Some(1), vec![], vec![held(false)]),
    ];
    let b = vec![
        smr_rec_full(1, Some(1), vec![], vec![held(true)]),
        smr_rec_full(2, Some(1), vec![], vec![held(false)]),
    ];
    let out = aggregate_smr(&[a, b], &smr_spec(), None).expect("aggregate");
    let lm = &out.landmarks[0];
    assert_eq!(
        lm.all_logs_round, None,
        "the simulator's coverage latch needs the log"
    );
    assert_eq!(
        lm.prefix_fixed_round,
        Some(1),
        "T_E still sees the executed copy"
    );
}

#[test]
fn client_log_sums_failures_and_keeps_the_command_line() {
    let log = ClientLog::parse(
        "{\"r\":1,\"arrivals\":2}\n{\"r\":1,\"sent\":5}\n{\"r\":1,\"lost\":1}\n{\"r\":1,\"starved\":0}\n\
         {\"r\":2,\"arrivals\":0}\n{\"r\":2,\"sent\":1}\n{\"r\":2,\"lost\":2}\n{\"r\":2,\"starved\":4}\n{\"commands\":[]}",
    )
    .expect("parse");
    assert_eq!(log.arrivals, vec![2, 0]);
    assert_eq!((log.lost, log.starved, log.sent), (3, 4, 6));
    assert_eq!(log.commands, "{\"commands\":[]}");
    assert!(ClientLog::parse("{\"r\":1,\"typo\":1}").is_err());
}

fn client_log(sent: u64) -> network_node::collect::ClientLog {
    network_node::collect::ClientLog {
        arrivals: vec![0, 0],
        lost: 0,
        starved: 0,
        sent,
        commands: String::new(),
    }
}

#[test]
fn client_sends_reconcile_against_received_late_early_and_blocked_commands() {
    let mut records = vec![
        vec![
            smr_rec(1, Some(1), 0, Some(1)),
            smr_rec(2, Some(1), 0, Some(1)),
        ],
        vec![
            smr_rec(1, Some(1), 0, Some(1)),
            smr_rec(2, Some(1), 0, Some(1)),
        ],
    ];
    records[0][0].cli_recv = 3;
    records[0][1].late_cli = 1;
    records[1][0].early_cli = 1;
    records[1][1].cli_blocked = 2;
    let out = aggregate_smr(&records, &smr_spec(), Some(&client_log(7))).expect("aggregate");
    assert_eq!(out.gates.cli_unreconciled, 0);
    assert_eq!((out.gates.late_cli, out.gates.early_cli), (1, 1));
    let out = aggregate_smr(&records, &smr_spec(), Some(&client_log(9))).expect("aggregate");
    assert_eq!(out.gates.cli_unreconciled, 2);
    assert!(!out.gates.network_clean());
}

#[test]
fn a_run_without_client_data_has_nothing_to_reconcile() {
    let mut records = vec![vec![smr_rec(1, Some(1), 0, Some(1))]];
    records[0][0].cli_recv = 3;
    let out = aggregate_smr(&records, &smr_spec_rounds(1), None).expect("aggregate");
    assert_eq!(out.gates.cli_unreconciled, 0);
}

#[test]
fn an_sn_order_violation_fails_the_safety_check_but_not_the_network_gates() {
    let mut records = vec![
        vec![
            smr_rec(1, Some(1), 0, Some(1)),
            smr_rec(2, Some(1), 0, Some(1)),
        ],
        vec![
            smr_rec(1, Some(1), 0, Some(1)),
            smr_rec(2, Some(1), 0, Some(1)),
        ],
    ];
    records[1][1].sn_order_violation = true;
    let out = aggregate_smr(&records, &smr_spec(), None).expect("aggregate");
    assert!(!out.gates.safety_ok);
    assert!(out.gates.network_clean(), "{:?}", out.gates);
}
