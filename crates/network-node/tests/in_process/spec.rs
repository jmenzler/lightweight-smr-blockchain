//! NodeRunSpec validation edges (outline §1 of the edge-case checklist).

use network_node::spec::NodeRunSpec;

fn gossip_json(holders: usize, x: u64, x0: u64) -> String {
    format!(
        r#"{{"scenario":{{"gossip":{{"n":16,"seed":1,"k":6,"ell":3,
             "holders":{holders},"x":{x},"x0":{x0},"max_rounds":40}}}},"sync":"timer"}}"#
    )
}

#[test]
fn defaults_fill_in() {
    let s = NodeRunSpec::from_json(&gossip_json(4, 7, 0)).expect("spec");
    assert_eq!(s.listen_port, 9000);
    assert_eq!(s.max_frame_bytes, 64 * 1024 * 1024);
    assert_eq!(s.payload_bytes, 0);
    assert_eq!(s.network.latency_ms, 0);
    assert!(s.tracked_ops.is_empty());
    assert_eq!(s.start_offset_ms, 0);
}

#[test]
fn gossip_rejects_bad_holder_counts_and_equal_values() {
    assert!(
        NodeRunSpec::from_json(&gossip_json(17, 7, 0)).is_err(),
        "holders > n"
    );
    assert!(
        NodeRunSpec::from_json(&gossip_json(0, 7, 0)).is_err(),
        "zero holders spread nothing"
    );
    assert!(
        NodeRunSpec::from_json(&gossip_json(4, 5, 5)).is_err(),
        "x == x0 is meaningless"
    );
}

#[test]
fn structural_knobs_are_validated() {
    let ok = gossip_json(4, 7, 0);
    let zero_round = ok.replace(r#""sync":"timer""#, r#""sync":"timer","round_ms":0"#);
    assert!(NodeRunSpec::from_json(&zero_round).is_err(), "round_ms 0");
    let tiny_frame = ok.replace(
        r#""sync":"timer""#,
        r#""sync":"timer","max_frame_bytes":16"#,
    );
    assert!(
        NodeRunSpec::from_json(&tiny_frame).is_err(),
        "frame cap below any message"
    );
    let zero_port = ok.replace(r#""sync":"timer""#, r#""sync":"timer","listen_port":0"#);
    assert!(NodeRunSpec::from_json(&zero_port).is_err(), "port 0");
}

#[test]
fn smr_validation_is_reached_through_the_arm() {
    // op 0 is reserved in the sim SMR family — must be rejected via TryFrom.
    let json = r#"{"scenario":{"smr":{"n":8,"seed":1,"k":6,"ell":3,"sigma":1.0,
        "proto":{"kind":"extended"},
        "injections":[{"round":1,"client":1,"op":0}],
        "max_rounds":20}},"sync":"timer"}"#;
    assert!(NodeRunSpec::from_json(json).is_err(), "reserved op 0");
}

#[test]
fn smr_rejects_a_pinned_target_out_of_range() {
    // sim::smr::SmrScenario::validate rejects target >= n; the node spec
    // arm reuses that TryFrom, so this must reach the same rejection.
    let json = r#"{"scenario":{"smr":{"n":3,"seed":1,"k":6,"ell":3,"sigma":1.0,
        "proto":{"kind":"extended"},
        "injections":[{"round":1,"client":1,"op":9,"target":3}],
        "max_rounds":20}},"sync":"timer"}"#;
    assert!(
        NodeRunSpec::from_json(json).is_err(),
        "target 3 is out of range for n=3"
    );
}

#[test]
fn priority_arm_validates_config_and_schedule() {
    let bad_ell = r#"{"scenario":{"priority":{"n":8,"seed":1,"k":6,"ell":2,
        "init":{"kind":"distinct"},"max_rounds":20}},"sync":"timer"}"#;
    assert!(NodeRunSpec::from_json(bad_ell).is_err(), "even ell");
    let bad_frac = r#"{"scenario":{"priority":{"n":8,"seed":1,"k":6,"ell":3,
        "init":{"kind":"distinct"},"max_rounds":20,
        "schedule":{"kind":"fresh_per_round","fraction":1.5}}},"sync":"timer"}"#;
    assert!(NodeRunSpec::from_json(bad_frac).is_err(), "fraction > 1");
}

fn smr_json(extra: &str) -> String {
    format!(
        r#"{{"scenario":{{"smr":{{"n":8,"seed":1,"k":6,"ell":3,"sigma":1.0,
        "proto":{{"kind":"compact","t_commit_rounds":10}},
        "injections":[{{"round":1,"client":1,"op":9}}],
        "max_rounds":20{extra}}}}},"sync":"timer"}}"#
    )
}

#[test]
fn smr_baseline_spec_is_accepted() {
    NodeRunSpec::from_json(&smr_json("")).expect("plain compact spec");
}

#[test]
fn smr_rejects_sim_only_fields_the_node_does_not_implement() {
    let cases = [
        (r#","partition":[0,0,0,0,1,1,1,1]"#, "partition"),
        (
            r#","manual_blocks":[{"node":0,"from_round":1}]"#,
            "manual_blocks",
        ),
        (
            r#","client_model":{"kind":"pool","clients":4}"#,
            "pool clients",
        ),
        (r#","certs":true"#, "certs"),
        (r#","halt_on_violation":true"#, "halt_on_violation"),
        (
            r#","merge_policy":{"stamp_tie":"commands_only"}"#,
            "merge_policy",
        ),
        (
            r#","schedule":{"kind":"adaptive_0late","fraction":0.1,"policy":"block_holders"}"#,
            "adaptive_0late",
        ),
        (
            r#","schedule":{"kind":"adaptive_alpha_late","fraction":0.1,"alpha":2,"policy":"block_holders"}"#,
            "adaptive_alpha_late",
        ),
    ];
    for (extra, what) in cases {
        let err = NodeRunSpec::from_json(&smr_json(extra)).expect_err(what);
        assert!(err.contains("networked mode"), "{what}: {err}");
    }
}

#[test]
fn smr_rejects_recovery_proto() {
    let json = smr_json("").replace(
        r#"{"kind":"compact","t_commit_rounds":10}"#,
        r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}"#,
    );
    let err = NodeRunSpec::from_json(&json).expect_err("recovery");
    assert!(err.contains("networked mode"), "{err}");
}

#[test]
fn value_arms_reject_partition_and_adaptive_schedules() {
    let median_partition = r#"{"scenario":{"median":{"n":8,"seed":1,"k":6,"ell":3,
        "init":{"kind":"distinct"},"max_rounds":20,
        "partition":[0,0,0,0,1,1,1,1]}},"sync":"timer"}"#;
    let err = NodeRunSpec::from_json(median_partition).expect_err("median partition");
    assert!(err.contains("networked mode"), "{err}");
    let gossip_adaptive = gossip_json(4, 7, 0).replace(
        r#""max_rounds":40"#,
        r#""max_rounds":40,"schedule":{"kind":"adaptive_0late","fraction":0.1,"policy":"block_holders"}"#,
    );
    let err = NodeRunSpec::from_json(&gossip_adaptive).expect_err("gossip adaptive");
    assert!(err.contains("networked mode"), "{err}");
    let priority_adaptive = r#"{"scenario":{"priority":{"n":8,"seed":1,"k":6,"ell":3,
        "init":{"kind":"distinct"},"max_rounds":20,
        "schedule":{"kind":"adaptive_alpha_late","fraction":0.1,"alpha":2,"policy":"block_holders"}}},"sync":"timer"}"#;
    let err = NodeRunSpec::from_json(priority_adaptive).expect_err("priority adaptive");
    assert!(err.contains("networked mode"), "{err}");
    let priority_partition = median_partition.replace("median", "priority");
    let err = NodeRunSpec::from_json(&priority_partition).expect_err("priority partition");
    assert!(err.contains("partition is not supported"), "{err}");
}

#[test]
fn engine_construction_validates_a_spec_that_skipped_from_json() {
    let priority_undecided = r#"{"scenario":{"priority":{"n":8,"seed":1,"k":6,"ell":3,
        "init":{"kind":"with_undecided","useful_fraction":0.5,"inner":{"kind":"distinct"}},
        "max_rounds":20}},"sync":"timer"}"#;
    let no_holders = r#"{"scenario":{"gossip":{"n":8,"seed":1,"k":6,"ell":3,
        "holders":0,"x":7,"x0":0,"max_rounds":20}},"sync":"timer"}"#;
    for json in [priority_undecided, no_holders] {
        let spec: NodeRunSpec = serde_json::from_str(json).expect("parses without validation");
        assert!(
            network_node::engine::RoundEngine::new(&spec, 0).is_err(),
            "hand-built spec must be refused, not run or panic"
        );
    }
}

#[test]
fn k_beyond_the_wire_slot_width_is_rejected() {
    let json = |k: usize| {
        format!(
            r#"{{"scenario":{{"median":{{"n":16,"seed":1,"k":{k},"ell":3,
            "init":{{"kind":"distinct"}},"max_rounds":5}}}},"sync":"timer"}}"#
        )
    };
    assert!(NodeRunSpec::from_json(&json(65_535)).is_ok());
    assert!(NodeRunSpec::from_json(&json(65_536)).is_err());
}

#[test]
fn disjoint_is_accepted_because_its_masks_need_no_node_state() {
    let json = smr_json(r#","schedule":{"kind":"disjoint","fraction":0.25}"#);
    let spec = NodeRunSpec::from_json(&json).expect("disjoint is a history-only schedule");
    network_node::engine::RoundEngine::new(&spec, 0).expect("engine precomputes the masks");
    let err = NodeRunSpec::from_json(&smr_json(
        r#","schedule":{"kind":"disjoint","fraction":0.6}"#,
    ))
    .expect_err("fraction above 1/2");
    assert!(err.contains("fraction <= 1/2"), "{err}");
}

#[test]
fn value_arms_reject_the_noise_controller_because_it_reads_node_state() {
    for arm in ["median", "priority"] {
        let json = format!(
            r#"{{"scenario":{{"{arm}":{{"n":50,"seed":1,"k":6,"ell":3,
            "init":{{"kind":"uniform_random","k":2}},"max_rounds":20,
            "schedule":{{"kind":"noise_controller","fraction":0.1}}}}}},"sync":"timer"}}"#
        );
        let err = NodeRunSpec::from_json(&json).expect_err(arm);
        assert!(err.contains("networked mode"), "{arm}: {err}");
    }
}
