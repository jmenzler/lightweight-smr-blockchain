//! Oracle 5: the recovery chain export the lab's blockchain panel draws.
//! The panel claims real hashes over the real committed sequence, so what is
//! checkable lives here — an independent forest twin, monotone length across a
//! surge, purity, and delta stitching.

use protocol::certificates::{MmrForest, ServerCertState, leaf_hash};
use protocol::compact::Entry;
use sim::smr::{ClientModel, SmrScenario, SmrState};
use sim::spec::SmrScenarioSpec;
use sim_wasm::{LiveSmr, SpillView};

fn recovery_spec(n: usize, t_window: u64) -> String {
    format!(
        r#"{{"n":{n},"seed":11,"k":6,"ell":3,"sigma":5.0,
            "proto":{{"kind":"recovery","t_window_rounds":{t_window},"resend_until_acked":false}},
            "injections":[{{"round":2,"client":1,"op":7}}],
            "max_rounds":200,
            "traffic":{{"kind":"pmf","arrivals_pmf":[0.4,0.6]}},
            "schedule":{{"kind":"fresh_per_round","fraction":0.0}}}}"#
    )
}

fn chain(live: &LiveSmr, from_len: u64) -> serde_json::Value {
    let value: serde_json::Value =
        serde_json::from_str(&live.chain_json(from_len)).expect("chain json");
    assert!(value["error"].is_null(), "chain failed: {}", value["error"]);
    value
}

/// The same engine session LiveSmr drives, stepped in lockstep — non-sticky
/// `step` and `step_fraction` consume the stream identically, so the twin's
/// `committed_entries` is the session's own sequence.
fn twin_of(spec_json: &str) -> SmrState {
    let spec: SmrScenarioSpec = serde_json::from_str(spec_json).expect("spec parses");
    let scenario = SmrScenario::try_from(spec).expect("valid scenario");
    SmrState::new(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &scenario.injections,
        scenario.traffic.as_deref().unwrap_or(&[]),
        ClientModel::Unique,
        None,
    )
}

fn hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn chain_json_forest_twins_an_independent_mmr_over_committed_entries() {
    let spec = recovery_spec(32, 10);
    let mut live = LiveSmr::new(&spec).expect("session");
    let mut twin = twin_of(&spec);
    for round in 1..=60 {
        live.step(0.0);
        twin.step_fraction(0.0);
        let entries = twin.committed_entries().expect("recovery sequence");

        let mut forest = MmrForest::new();
        for entry in &entries {
            forest.append(leaf_hash(entry));
        }
        let expected: Vec<serde_json::Value> = forest
            .peaks()
            .iter()
            .map(|(height, root, start)| {
                serde_json::json!({ "height": height, "root_hex": hex(root), "start": start })
            })
            .collect();

        let snapshot = chain(&live, 0);
        assert_eq!(snapshot["m"], entries.len(), "round {round}: length");
        assert_eq!(
            snapshot["peaks"],
            serde_json::Value::Array(expected),
            "round {round}: peaks/roots diverge from an independent forest"
        );
    }
}

#[test]
fn committed_length_never_decreases_across_a_surge_release_and_drain() {
    let mut live = LiveSmr::new(&recovery_spec(32, 10)).expect("session");
    let mut last = 0u64;
    let mut peak_before_surge = 0u64;
    for round in 1..=100 {
        // full blackout 35–62, exactly the arc the lab's surge preset drives
        let fraction = if (35..=62).contains(&round) { 1.0 } else { 0.0 };
        live.step(fraction);
        let m = chain(&live, 0)["m"].as_u64().expect("m");
        assert!(
            m >= last,
            "round {round}: committed length fell {last} -> {m}"
        );
        if round == 34 {
            peak_before_surge = m;
        }
        last = m;
    }
    assert!(peak_before_surge > 0, "commits must land before the surge");
    assert!(last > peak_before_surge, "the drain must commit again");
}

#[test]
fn chain_json_is_pure_and_repeatable() {
    let mut live = LiveSmr::new(&recovery_spec(32, 10)).expect("session");
    for _ in 0..40 {
        live.step(0.0);
    }
    // Repeated reads are byte-identical, and reading does not disturb the
    // session: the next step matches an untouched twin's.
    let first = live.chain_json(0);
    assert_eq!(first, live.chain_json(0));
    assert_eq!(live.chain_json(3), live.chain_json(3));

    let mut quiet = LiveSmr::new(&recovery_spec(32, 10)).expect("session");
    for _ in 0..40 {
        quiet.step(0.0);
    }
    assert_eq!(live.step(0.1), quiet.step(0.1));
}

#[test]
fn successive_deltas_stitch_into_the_full_sequence() {
    let mut live = LiveSmr::new(&recovery_spec(32, 10)).expect("session");
    let mut stitched: Vec<serde_json::Value> = Vec::new();
    for _ in 1..=60 {
        live.step(0.0);
        let delta = chain(&live, stitched.len() as u64);
        stitched.extend(delta["entries"].as_array().expect("entries").clone());
        assert_eq!(
            stitched.len() as u64,
            delta["m"].as_u64().expect("m"),
            "the delta must close the gap exactly"
        );
    }
    let full = chain(&live, 0);
    assert_eq!(
        serde_json::Value::Array(stitched),
        full["entries"],
        "accumulated deltas must equal one full read"
    );
    // A cursor past the end is an empty delta, never an error.
    let past = chain(&live, full["m"].as_u64().expect("m") + 50);
    assert_eq!(past["entries"], serde_json::json!([]));
    assert_eq!(past["m"], full["m"]);
    assert_eq!(past["peaks"], full["peaks"]);
}

#[test]
fn chain_entries_label_every_committed_position() {
    let mut live = LiveSmr::new(&recovery_spec(32, 10)).expect("session");
    let mut twin = twin_of(&recovery_spec(32, 10));
    for _ in 0..60 {
        live.step(0.0);
        twin.step_fraction(0.0);
    }
    let entries = twin.committed_entries().expect("recovery sequence");
    let labelled = chain(&live, 0);
    let rows = labelled["entries"].as_array().expect("entries");
    assert_eq!(rows.len(), entries.len());
    for (row, entry) in rows.iter().zip(&entries) {
        match entry {
            Entry::Cmd(cc) => {
                assert_eq!(row["kind"], "cmd");
                assert_eq!(row["client"], cc.client);
                assert_eq!(row["sn"], cc.sn);
                assert_eq!(row["op"], cc.op);
            }
            Entry::Null { client, sn } => {
                assert_eq!(row["kind"], "null");
                assert_eq!(row["client"], *client);
                assert_eq!(row["sn"], *sn);
            }
            Entry::Nop(op) => {
                assert_eq!(row["kind"], "nop");
                assert_eq!(row["op"], *op);
            }
        }
    }
    assert!(
        rows.iter().any(|r| r["kind"] == "cmd"),
        "the run must commit client commands, not only structure"
    );
}

#[test]
fn chain_json_errors_outside_the_recovery_rule() {
    for proto in [
        r#"{"kind":"extended"}"#,
        r#"{"kind":"compact","t_commit_rounds":6}"#,
    ] {
        let spec = format!(
            r#"{{"n":16,"seed":4,"k":6,"ell":3,"sigma":1.5,"proto":{proto},
                "injections":[{{"round":2,"client":1,"op":7}}],"max_rounds":30,
                "schedule":{{"kind":"fresh_per_round","fraction":0.1}}}}"#
        );
        let mut live = LiveSmr::new(&spec).expect("session");
        for _ in 0..10 {
            live.step(0.1);
        }
        let value: serde_json::Value =
            serde_json::from_str(&live.chain_json(0)).expect("chain json");
        assert!(
            value["error"]
                .as_str()
                .expect("error string")
                .contains("recovery"),
            "{proto}: expected a wrong-rule error, got {value}"
        );
    }
}

// --- RQ11: certs_json over the recovery rule (ack mode only) ---

fn cert_spec(resend_until_acked: bool) -> String {
    format!(
        r#"{{"n":32,"seed":11,"k":6,"ell":3,"sigma":5.0,
            "proto":{{"kind":"recovery","t_window_rounds":10,
                      "resend_until_acked":{resend_until_acked}}},
            "injections":[{{"round":2,"client":1,"op":7}},
                          {{"round":40,"client":1,"op":8}}],
            "max_rounds":200,
            "schedule":{{"kind":"fresh_per_round","fraction":0.0}}}}"#
    )
}

#[test]
fn certs_json_serves_the_recovery_construction_when_ack_mode_is_on() {
    let mut live = LiveSmr::new(&cert_spec(true)).expect("session");
    for _ in 0..100 {
        live.step(0.0);
    }
    let snap: serde_json::Value = serde_json::from_str(&live.certs_json()).expect("certs json");
    assert!(snap["error"].is_null(), "certs failed: {}", snap["error"]);
    assert_eq!(snap["roots_consistent"], serde_json::json!(true));

    let servers = snap["servers"].as_array().expect("servers");
    assert_eq!(servers.len(), 32);
    let m = servers[0]["m"].as_u64().expect("m");
    assert!(m > 0, "the checkpoint forest must have committed something");
    assert!(
        servers.iter().all(|s| s["m"] == serde_json::json!(m)),
        "every server carries the same checkpoint forest in a clean run"
    );

    // The §5 command forest and the chain panel's display forest coincide:
    // no filler ever commits, so peaks and positions are the same object.
    let chain = chain(&live, 0);
    assert_eq!(chain["m"], servers[0]["m"], "the two forests disagree on m");
    assert_eq!(chain["peaks"], servers[0]["peaks"], "peak sets diverged");

    // Engine-side positions are 0-based (array index); the lab displays +1.
    let committed = snap["committed"].as_array().expect("committed");
    assert_eq!(committed.len() as u64, m);
    assert_eq!(committed[0]["kind"], serde_json::json!("cmd"));
    assert_eq!(committed[0]["sn"], serde_json::json!(1));

    let clients = snap["clients"].as_array().expect("clients");
    let one = clients
        .iter()
        .find(|c| c["client"] == serde_json::json!(1))
        .expect("client 1 row");
    assert!(
        one["next_sn"].as_u64().expect("next_sn") >= 2,
        "the client must have been acked at least once"
    );
    let rows = one["certs"].as_array().expect("cert rows");
    assert_eq!(rows[0]["committed"], serde_json::json!(true));
    assert_eq!(rows[0]["pos"], serde_json::json!(0), "0-based engine-side");
}

#[test]
fn certs_json_errors_on_recovery_without_ack_mode() {
    // Without an ack moment no notification can ever be delivered, so the
    // panel must say so rather than draw an empty client table.
    let mut live = LiveSmr::new(&cert_spec(false)).expect("session");
    for _ in 0..100 {
        live.step(0.0);
    }
    let snap: serde_json::Value = serde_json::from_str(&live.certs_json()).expect("certs json");
    assert!(
        snap["error"]
            .as_str()
            .expect("error string")
            .contains("acked"),
        "unexpected error envelope: {}",
        snap["error"]
    );
}

#[test]
fn exported_spec_round_trips_ack_mode_in_both_settings() {
    // Provenance: a cert-bearing session must replay as one. The client mode
    // is a required spec field, so the export names it in both settings.
    let mut live = LiveSmr::new(&cert_spec(true)).expect("session");
    for _ in 0..12 {
        live.step(0.0);
    }
    let exported: serde_json::Value =
        serde_json::from_str(&live.export_scenario()).expect("exported spec");
    assert_eq!(
        exported["proto"]["resend_until_acked"],
        serde_json::json!(true)
    );
    let replayed: SmrScenarioSpec =
        serde_json::from_value(exported).expect("exported spec re-parses");
    assert!(matches!(
        SmrScenario::try_from(replayed).expect("valid").proto,
        sim::smr::Proto::Recovery {
            resend_until_acked: true,
            ..
        }
    ));

    let mut off = LiveSmr::new(&cert_spec(false)).expect("session");
    off.step(0.0);
    let exported: serde_json::Value =
        serde_json::from_str(&off.export_scenario()).expect("exported spec");
    assert_eq!(
        exported["proto"]["resend_until_acked"],
        serde_json::json!(false),
        "an off flag is exported explicitly"
    );
    let replayed: SmrScenarioSpec =
        serde_json::from_value(exported).expect("exported spec re-parses");
    assert!(matches!(
        SmrScenario::try_from(replayed).expect("valid").proto,
        sim::smr::Proto::Recovery {
            resend_until_acked: false,
            ..
        }
    ));
}

#[test]
fn the_labs_forced_cert_layer_never_reaches_the_exported_spec() {
    // The lab mounts §5 at engine construction so its panels work with no JS
    // change; the export is the operator's own spec, so the same session
    // replays on the grid gated OFF — which the row-identity pin makes exact.
    let mut live = LiveSmr::new(&cert_spec(true)).expect("session");
    for _ in 0..40 {
        live.step(0.0);
    }
    let snap: serde_json::Value = serde_json::from_str(&live.certs_json()).expect("certs json");
    assert!(
        snap["error"].is_null(),
        "the lab must keep its cert panel: {}",
        snap["error"]
    );

    let exported = live.export_scenario();
    assert!(
        !exported.contains("certs"),
        "the forced layer leaked into the exported spec: {exported}"
    );
    let replayed: SmrScenarioSpec =
        serde_json::from_str(&exported).expect("exported spec re-parses");
    assert!(
        !SmrScenario::try_from(replayed).expect("valid").certs,
        "a replay of a lab export must run Alg 6 as printed"
    );
}

// --- the spill viewer: the same chain panel over a run that forgot ---

/// The lab's recovery chain panel is the one view with a real dependency on
/// the WHOLE committed history from position 0, and a lean run's servers no
/// longer hold it. This is the acceptance check for the viewer that replaces
/// them: same seed, lean+spill against full-history, at the Rust/wasm level
/// where it is checkable — the JS layer is then a renderer over data that has
/// already been compared.
fn forgetting_scenario() -> SmrScenario {
    SmrScenario {
        n: 32,
        seed: 4_340_000,
        cfg: sim::Config::default(),
        sigma: 1.0,
        proto: sim::smr::Proto::Recovery {
            t_window_rounds: 20,
            resend_until_acked: false,
            prefix_mismatch: sim::smr::PrefixMismatch::Abort,
        },
        injections: vec![],
        max_rounds: 200,
        schedule: sim::BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: Some(vec![sim::smr::TrafficPhase {
            from_round: 1,
            arrivals_pmf: sim::smr::point_mass_pmf(4),
        }]),
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    }
}

#[test]
fn the_spill_viewer_reconstructs_the_chain_a_lean_run_no_longer_holds() {
    let scenario = forgetting_scenario();
    let lean = sim::smr::run_smr_lean(&scenario);
    let path = lean.spill_path.clone().expect(
        "a lean recovery run must write a spill — unset SIM_SPILL and SIM_RUNLOG to run this",
    );

    // The full-history reference, stepped rather than batched so the per-node
    // views at every boundary are observable. That it IS the same run is not
    // assumed: the reports have to agree.
    let mut twin = SmrState::new(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &scenario.injections,
        scenario.traffic.as_deref().expect("traffic"),
        ClientModel::Unique,
        None,
    );
    let mut per_boundary: Vec<Vec<Vec<Entry>>> = Vec::new();
    for round in 1..=scenario.max_rounds {
        twin.step_sampled(0);
        if round % 20 == 0 {
            per_boundary.push(
                (0..scenario.n)
                    .map(|i| {
                        twin.executed_entries(i)
                            .expect("recovery executed seq")
                            .to_vec()
                    })
                    .collect(),
            );
        }
    }
    // Two claims, kept apart. First, the stepped reference IS the batch run —
    // same stream, same report. Second, the lean run is the same HISTORY: the
    // per-round counters carry min/max executed length every round and the
    // recovery block carries the lineage oracle, so agreement there is
    // agreement about what was committed and when. (Lean and full also agree
    // on every landmark, but that is `smr_lean_report`'s pin to make, not this
    // one's; here the lean run legitimately sheds settled spread curves.)
    let reference = twin.report();
    assert_eq!(
        reference,
        sim::smr::run_smr(&scenario),
        "the stepped reference is not the batch run"
    );
    assert_eq!(
        lean.metrics, reference.metrics,
        "the lean run committed a different history than the reference"
    );
    assert_eq!(lean.recovery, reference.recovery);
    assert_eq!(lean.terminal, reference.terminal);
    assert!(lean.safety_ok && reference.safety_ok);

    let text = std::fs::read_to_string(&path).expect("the spill reads back");
    let view = SpillView::new(&text).expect("the spill parses");
    let summary: serde_json::Value =
        serde_json::from_str(&view.summary_json()).expect("summary json");
    assert_eq!(summary["verified"], serde_json::json!(true));
    assert_eq!(summary["complete"], serde_json::json!(true));
    assert_eq!(summary["diverged"], serde_json::json!(false));
    assert_eq!(summary["n"], serde_json::json!(scenario.n));
    assert_eq!(
        summary["boundaries"].as_array().expect("boundaries").len(),
        per_boundary.len()
    );

    // The forest over each reconstructed view, against an INDEPENDENT MMR over
    // what the full-history run actually committed. Independent on purpose:
    // comparing the viewer's forest against the same function that built it
    // would only prove the function is deterministic.
    let mut checked = 0;
    for (b, per_node) in per_boundary.iter().enumerate() {
        for (i, expected) in per_node.iter().enumerate() {
            let chain: serde_json::Value =
                serde_json::from_str(&view.chain_json(b, i, 0)).expect("chain json");
            assert!(
                chain["error"].is_null(),
                "boundary {b}, node {i}: {}",
                chain["error"]
            );
            assert_eq!(
                chain["m"],
                serde_json::json!(expected.len()),
                "boundary {b}, node {i}: committed length"
            );
            let mut forest = MmrForest::new();
            for entry in expected {
                forest.append(leaf_hash(entry));
            }
            let peaks: Vec<serde_json::Value> = forest
                .peaks()
                .iter()
                .map(|(height, root, start)| {
                    serde_json::json!({
                        "height": height,
                        "root_hex": hex(root),
                        "start": start,
                    })
                })
                .collect();
            assert_eq!(
                chain["peaks"],
                serde_json::Value::Array(peaks),
                "boundary {b}, node {i}: the reconstructed forest is not the \
                 forest over what the run committed"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "the shape produced no boundaries to check");
    assert!(
        per_boundary
            .last()
            .expect("boundaries")
            .iter()
            .any(|s| s.len() > 10),
        "the shape committed almost nothing, so the reconstruction pins little"
    );
}

/// A pooled twin of [`forgetting_scenario`]: same shape, but arrivals draw from
/// a bounded client set, so clients commit SEVERAL commands each. That is what
/// p. 28's "last two commands" rule needs in order to bite at all — under the
/// `Unique` model every client commits exactly once, the second window slot
/// never fills, and a last-two reconstruction would be a last-one
/// reconstruction wearing its name.
fn pooled_forgetting_scenario() -> SmrScenario {
    SmrScenario {
        // Recovery commits only at boundaries, so in-flight commands pile up
        // for up to two T-windows: at rate 2 with T = 20 that is ~80, and the
        // pool has to clear it or the run aborts on exhaustion rather than
        // measuring anything.
        client_model: ClientModel::Pool { clients: 160 },
        traffic: Some(vec![sim::smr::TrafficPhase {
            from_round: 1,
            arrivals_pmf: sim::smr::point_mass_pmf(2),
        }]),
        // The recovery T-floor is LOAD-dependent, not just T_B: amplification
        // has to clear it or commitment stalls and the boundary strip aborts.
        sigma: 5.0,
        ..forgetting_scenario()
    }
}

/// The OTHER half of pp. 27–28, and the one that licenses gating §5 storage
/// off at all: a useful server must store the forest root hashes (p. 27) AND,
/// per client, the hash chains of its last two committed commands (p. 28). The
/// forest half is reconstructed above. This pins the window half, so
/// "post-hoc recomputation is byte-exact" is verified for the whole storage
/// list rather than half of it.
///
/// What it is and is not. It is a RECOMPUTABILITY check: the windows a
/// cert-bearing run held are recoverable from a spill written by a run that
/// stored none, which is the claim the §5 gate rests on. It is NOT an
/// independent check of `ServerCertState::append` — both sides fold through
/// that one function, deliberately, because the question is whether the
/// committed stream carries enough information, not whether the fold is right.
/// `recovery_rule`'s forest-twin invariant is what audits the fold.
#[test]
fn the_spill_reconstructs_the_per_client_certificate_windows_not_just_the_forest() {
    let scenario = pooled_forgetting_scenario();
    assert!(!scenario.certs, "the spill side must run gated off");
    let lean = sim::smr::run_smr_lean(&scenario);
    let path = lean.spill_path.clone().expect(
        "a lean recovery run must write a spill — unset SIM_SPILL and SIM_RUNLOG to run this",
    );

    // The cert-ON reference. Same seed and same scenario: the gate is inert in
    // every reported field, which the row-identity pin establishes and the
    // report comparison below re-establishes on this shape.
    let mut twin = SmrState::new_with_certs(
        scenario.n,
        scenario.cfg,
        scenario.proto,
        scenario.sigma,
        scenario.seed,
        &scenario.injections,
        scenario.traffic.as_deref().expect("traffic"),
        scenario.client_model,
        None,
        true,
        scenario.merge_policy,
        scenario.repeated_commit,
    );
    let mut live: Vec<Vec<ServerCertState>> = Vec::new();
    for round in 1..=scenario.max_rounds {
        twin.step_sampled(0);
        if round % 20 == 0 {
            live.push(
                (0..scenario.n)
                    .map(|i| {
                        twin.checkpoint_certs(i)
                            .expect("the reference mounts §5")
                            .clone()
                    })
                    .collect(),
            );
        }
    }
    assert_eq!(
        lean.metrics,
        twin.report().metrics,
        "the gated-off run and the cert-bearing reference committed different histories"
    );

    let text = std::fs::read_to_string(&path).expect("the spill reads back");
    // The SAME reader the viewer calls — reconstruction exists once.
    let spill = sim::smr::Spill::parse(&text).expect("the spill parses");
    spill.verify().expect("the spill verifies");

    let mut clients_checked = 0usize;
    let mut windows_checked = 0usize;
    let mut chained = 0usize;
    let mut two_deep = 0usize;
    for (b, per_node) in live.iter().enumerate() {
        for (i, held) in per_node.iter().enumerate() {
            let view = spill.node_view(b, i).expect("a node view reconstructs");
            let mut rebuilt = ServerCertState::new();
            for entry in &view {
                rebuilt.append(entry);
            }
            assert_eq!(
                rebuilt.forest().roots(),
                held.forest().roots(),
                "boundary {b}, node {i}: the reconstructed forest is not the run's"
            );
            let clients: std::collections::BTreeSet<u32> = view
                .iter()
                .filter_map(|e| match e {
                    Entry::Cmd(c) => Some(c.client),
                    _ => None,
                })
                .collect();
            for client in clients {
                assert_eq!(
                    rebuilt.last_two(client),
                    held.last_two(client),
                    "boundary {b}, node {i}, client {client}: the reconstructed last-two \
                     window — leaf, position, sn and the whole Merkle chain — is not what the \
                     run stored"
                );
                let (prev, newest) = held.last_two(client).expect("just compared");
                chained += usize::from(!newest.chain.is_empty());
                two_deep += usize::from(prev.is_some());
                clients_checked += 1;
            }
            windows_checked += 1;
        }
    }
    assert!(
        windows_checked > 0 && clients_checked > windows_checked,
        "the shape pinned too little: {windows_checked} window maps over \
         {clients_checked} client windows"
    );
    // Anti-vacuity on the half this test exists for: a window whose chain is
    // empty is a leaf that never merged, and comparing those would pin nothing
    // the forest comparison above does not already pin. The Merkle paths are
    // what p. 28 actually requires a server to store.
    assert!(
        chained * 2 > clients_checked,
        "only {chained} of {clients_checked} reconstructed windows carry a Merkle chain, \
         so this pins the leaves rather than the paths"
    );
    // And on the rule itself: p. 28 stores the last TWO, so a run where no
    // client ever committed twice would reconstruct a one-slot window and
    // claim the two-slot requirement.
    assert!(
        two_deep * 3 > clients_checked,
        "only {two_deep} of {clients_checked} windows hold a previous command, so the \
         last-TWO rule is barely exercised"
    );
}

/// A viewer must never present an unverifiable file as sound. Corrupt one
/// committed entry and the summary has to say so — and still lay the run out,
/// because "we cannot trust this" is more useful than a blank page.
#[test]
fn a_corrupted_spill_is_reported_unverified_rather_than_drawn_as_sound() {
    let scenario = forgetting_scenario();
    let lean = sim::smr::run_smr_lean(&scenario);
    let path = lean.spill_path.expect("a lean recovery run writes a spill");
    let text = std::fs::read_to_string(&path).expect("the spill reads back");

    let clean: serde_json::Value =
        serde_json::from_str(&SpillView::new(&text).expect("parses").summary_json())
            .expect("summary json");
    assert_eq!(clean["verified"], serde_json::json!(true));

    // Flip one op in the first boundary's committed suffix.
    let corrupted = text.replacen("\"op\":", "\"op\":9999999,\"was_op\":", 1);
    let summary: serde_json::Value = serde_json::from_str(
        &SpillView::new(&corrupted)
            .expect("a corrupted file still parses")
            .summary_json(),
    )
    .expect("summary json");
    assert_eq!(
        summary["verified"],
        serde_json::json!(false),
        "a corrupted stream must not report as verified"
    );
    assert!(
        summary["verify_error"]
            .as_str()
            .is_some_and(|e| e.contains("boundary")),
        "the report must name the boundary that failed: {}",
        summary["verify_error"]
    );
}
