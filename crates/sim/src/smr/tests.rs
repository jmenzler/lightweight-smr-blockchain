use super::*;

// K is generous: these tests are about reuse bookkeeping, not K sizing.
fn loaded_pool_state(clients: u32, rounds: usize) -> SmrState {
    let n = 16;
    let mut state = SmrState::new(
        n,
        Config::default(),
        Proto::Compact { t_commit_rounds: 8 },
        1.0,
        7,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(2),
        }],
        ClientModel::Pool { clients },
        None,
    );
    let mask = vec![false; n];
    for _ in 0..rounds {
        state.draw_arrivals();
        state.step_masked(&mask);
    }
    state
}

// Asserts clients were actually reused, so callers cannot pass vacuously.
fn by_client(state: &SmrState) -> std::collections::BTreeMap<u32, Vec<&CommandTracker>> {
    let mut out: std::collections::BTreeMap<u32, Vec<&CommandTracker>> =
        std::collections::BTreeMap::new();
    for t in &state.trackers {
        out.entry(t.client).or_default().push(t);
    }
    assert!(
        out.values().any(|ts| ts.len() > 2),
        "the horizon must actually reuse clients"
    );
    out
}

fn compact_run(truncate: bool, rounds: usize, frozen: Option<usize>) -> SmrState {
    compact_run_at(32, truncate, rounds, frozen)
}

fn compact_run_at(n: usize, truncate: bool, rounds: usize, frozen: Option<usize>) -> SmrState {
    let mut state = SmrState::new(
        n,
        Config::default(),
        Proto::Compact {
            t_commit_rounds: 38,
        },
        1.0,
        975_300,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(4),
        }],
        ClientModel::Unique,
        None,
    );
    state.truncate_history = truncate;
    for _ in 0..rounds {
        state.draw_arrivals();
        let mut mask = state.sample_mask(state.blocked_count(0.1));
        if let Some(i) = frozen {
            mask[i] = true;
        }
        state.step_masked(&mask);
    }
    state
}

fn offsets(state: &SmrState) -> Vec<u64> {
    let SmrNodes::Comp(nodes) = &state.nodes else {
        panic!("compact run");
    };
    nodes
        .iter()
        .map(|nd| nd.shared_state().executed_offset())
        .collect()
}

fn logical_lens(state: &SmrState) -> Vec<u64> {
    let SmrNodes::Comp(nodes) = &state.nodes else {
        panic!("compact run");
    };
    nodes
        .iter()
        .map(|nd| nd.shared_state().logical_len())
        .collect()
}

#[test]
fn forgetting_the_committed_prefix_moves_no_landmark_and_no_position() {
    for frozen in [None, Some(3)] {
        let full = compact_run(false, 150, frozen);
        let cut = compact_run(true, 150, frozen);
        assert!(full.safety_ok(), "the pin covers consistent runs");
        assert!(
            offsets(&full).iter().all(|&o| o == 0),
            "the reference run must keep everything"
        );
        let cut_offsets = offsets(&cut);
        assert_eq!(
            cut_offsets.iter().any(|&o| o > 0),
            LEAN_FORGETS_HISTORY,
            "the ledger records LEAN_FORGETS_HISTORY = {LEAN_FORGETS_HISTORY}, \
                 which is not what a lean run did"
        );
        let lens = logical_lens(&cut);
        assert!(
            lens.iter().min() < lens.iter().max(),
            "the shape must spread the executed lengths: {lens:?}"
        );
        if let Some(i) = frozen {
            assert_eq!(
                cut_offsets[i], lens[i],
                "a frozen server is clamped at its own length, never past it"
            );
            assert!(
                lens[i] < *lens.iter().max().expect("nodes"),
                "node {i} did not actually freeze"
            );
        }
        assert_eq!(full.report(), cut.report(), "frozen: {frozen:?}");
    }
}

#[test]
fn forgetting_agrees_with_the_reference_run_at_campaign_shape() {
    let full = compact_run_at(64, false, 600, None);
    let cut = compact_run_at(64, true, 600, None);
    assert!(full.safety_ok(), "the pin covers consistent runs");
    let lens = logical_lens(&cut);
    let cut_offsets = offsets(&cut);
    let committed = *lens.iter().max().expect("nodes");
    let frontier = *cut_offsets.iter().max().expect("nodes");
    assert!(
        frontier > 200 && frontier * 2 > committed,
        "the frontier stayed near the origin: {frontier} of {committed}"
    );
    assert_eq!(full.report(), cut.report());
}

// `frozen` holds one server below the release line for the whole run.
fn recovery_state(truncate: bool) -> SmrState {
    let mut state = SmrState::new(
        32,
        Config::default(),
        Proto::Recovery {
            t_window_rounds: 20,
            resend_until_acked: false,
            prefix_mismatch: crate::smr::PrefixMismatch::Abort,
        },
        1.0,
        4_340_000,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(4),
        }],
        ClientModel::Unique,
        None,
    );
    state.truncate_history = truncate;
    state
}

fn recovery_run(truncate: bool, rounds: usize, frozen: Option<usize>) -> SmrState {
    let mut state = recovery_state(truncate);
    for _ in 0..rounds {
        state.draw_arrivals();
        let mut mask = state.sample_mask(state.blocked_count(0.1));
        if let Some(i) = frozen {
            mask[i] = true;
        }
        state.step_masked(&mask);
    }
    state
}

fn rec_nodes(state: &SmrState) -> &[RecoveryNode] {
    let SmrNodes::Rec(nodes) = &state.nodes else {
        panic!("recovery run");
    };
    nodes
}

fn rec_offsets(state: &SmrState) -> Vec<u64> {
    rec_nodes(state)
        .iter()
        .map(|nd| nd.shared_state().executed_offset())
        .collect()
}

fn rec_logical_lens(state: &SmrState) -> Vec<u64> {
    rec_nodes(state)
        .iter()
        .map(|nd| nd.shared_state().logical_len())
        .collect()
}

#[test]
fn recovery_forgets_its_committed_prefix_at_the_boundary() {
    for frozen in [None, Some(3)] {
        let full = recovery_run(false, 200, frozen);
        let cut = recovery_run(true, 200, frozen);
        assert!(full.safety_ok(), "the pin covers consistent runs");
        assert!(
            rec_offsets(&full).iter().all(|&o| o == 0),
            "the reference run must keep everything"
        );
        let cut_offsets = rec_offsets(&cut);
        assert_eq!(
            cut_offsets.iter().any(|&o| o > 0),
            LEAN_FORGETS_HISTORY,
            "the ledger records LEAN_FORGETS_HISTORY = {LEAN_FORGETS_HISTORY}, \
                 which is not what a lean recovery run did"
        );
        let lens = rec_logical_lens(&cut);
        assert!(
            lens.iter().min() < lens.iter().max(),
            "the shape must spread the executed lengths: {lens:?}"
        );
        for (i, (&offset, &len)) in cut_offsets.iter().zip(&lens).enumerate() {
            assert!(
                offset <= len,
                "node {i}: forgot past its own length ({offset} of {len})"
            );
        }
        if let Some(i) = frozen {
            assert!(
                lens[i] < *lens.iter().max().expect("nodes"),
                "node {i} did not actually freeze"
            );
        }
        // The spill path is provenance, not an observation; full-history runs write none.
        let (mut full_report, mut cut_report) = (full.report(), cut.report());
        assert!(
            full_report.spill_path.is_none(),
            "a full-history run must write no spill"
        );
        full_report.spill_path = None;
        cut_report.spill_path = None;
        assert_eq!(full_report, cut_report, "frozen: {frozen:?}");
    }
}

#[test]
fn a_lean_run_reconstructs_every_node_view_from_its_spill() {
    const T_WINDOW: usize = 20;
    let mut full = recovery_state(false);
    let mut cut = recovery_state(true);
    let mut expected: Vec<Vec<Vec<Entry>>> = Vec::new();
    for round in 1..=200 {
        full.draw_arrivals();
        cut.draw_arrivals();
        let mask = full.sample_mask(full.blocked_count(0.1));
        assert_eq!(
            mask,
            cut.sample_mask(cut.blocked_count(0.1)),
            "round {round}: the two runs drew different masks, so the \
                 reconstruction would be comparing two different runs"
        );
        full.step_masked(&mask);
        cut.step_masked(&mask);
        if round % T_WINDOW == 0 {
            expected.push(
                (0..full.n)
                    .map(|i| full.executed_entries(i).expect("recovery").to_vec())
                    .collect(),
            );
        }
    }
    cut.finish_spill();

    let path = cut.report().spill_path.expect(
        "a lean recovery run must write a spill — unset SIM_SPILL and SIM_RUNLOG \
             to run this test",
    );
    let spill = read_spill(std::path::Path::new(&path)).expect("the spill reads back");
    spill
        .verify()
        .expect("the spill must verify against its own chain");
    assert_eq!(
        spill.trailer.as_ref().map(|t| t.diverged),
        Some(false),
        "a clean run's trailer must say so"
    );
    assert_eq!(
        spill.boundaries.len(),
        expected.len(),
        "one boundary record per T-window boundary"
    );
    for (b, per_node) in expected.iter().enumerate() {
        for (i, seq) in per_node.iter().enumerate() {
            assert_eq!(
                &spill.node_view(b, i).expect("a view per node per boundary"),
                seq,
                "boundary {b}, node {i}: the reconstruction is not what the run committed"
            );
        }
    }
    assert!(
        expected
            .last()
            .expect("boundaries")
            .iter()
            .any(|s| !s.is_empty()),
        "the shape committed nothing, so the reconstruction pins nothing"
    );

    let canonical = spill.canonical_through(spill.boundaries.len() - 1);
    let mut stitched_any = false;
    for (i, nd) in rec_nodes(&cut).iter().enumerate() {
        let ex = nd.shared_state().executed();
        stitched_any |= ex.offset() > 0;
        let stitched: Vec<Entry> = canonical[..ex.offset() as usize]
            .iter()
            .chain(ex.iter_from(ex.offset()))
            .cloned()
            .collect();
        assert_eq!(
            stitched,
            full.executed_entries(i).expect("recovery").to_vec(),
            "node {i}: canonical[0..offset] ++ tail is not the full-history sequence"
        );
    }
    assert!(
        stitched_any,
        "no node had forgotten anything, so the stitch pinned nothing"
    );
}

#[test]
fn a_spill_off_run_names_the_missing_diagnosis_and_its_flag() {
    let state = protocol::compact::SharedState::from_entries(Vec::new(), Default::default());
    let report = observe::spill::diagnose(None, 3, 41, state.executed());
    assert!(report.contains("SIM_SPILL"), "{report}");
    assert!(report.contains("diagnosis unavailable"), "{report}");
    assert!(
        report.contains("seed"),
        "the message must say what to re-run: {report}"
    );
}

#[test]
fn the_recovery_residency_is_flat_in_the_horizon() {
    let residency = |state: &SmrState| {
        rec_nodes(state)
            .iter()
            .map(|nd| nd.shared_state().retained_len() + nd.checkpoint().s.retained_len())
            .max()
            .expect("nodes")
    };
    let committed = |state: &SmrState| rec_logical_lens(state).into_iter().max().expect("nodes");

    let short = recovery_run(true, 100, None);
    let long = recovery_run(true, 300, None);
    assert!(
        committed(&long) > 2 * committed(&short),
        "the longer run did not commit more: {} vs {}",
        committed(&long),
        committed(&short)
    );
    assert!(
        residency(&long) < 2 * residency(&short),
        "the retained history grew with the horizon: {} vs {}",
        residency(&long),
        residency(&short)
    );
}

#[test]
fn two_runs_differing_only_in_a_parameter_spill_to_distinct_files() {
    let spill_of = |tag: &str| {
        let mut state = recovery_state(true);
        state.scenario_tag = Some(tag.to_string());
        for _ in 1..=40 {
            state.draw_arrivals();
            let mask = state.sample_mask(state.blocked_count(0.1));
            state.step_masked(&mask);
        }
        state.finish_spill();
        state.report().spill_path.expect(
            "a lean recovery run must write a spill — unset SIM_SPILL and SIM_RUNLOG \
             to run this test",
        )
    };
    let (a, b) = (spill_of("aaaaaaaa"), spill_of("bbbbbbbb"));
    assert_ne!(a, b);
    assert!(a.ends_with("aaaaaaaa.jsonl"), "{a}");
    assert!(b.ends_with("bbbbbbbb.jsonl"), "{b}");
}

fn compact_run_shape() -> SmrState {
    SmrState::new(
        32,
        Config::default(),
        Proto::Compact {
            t_commit_rounds: 21,
        },
        1.0,
        975_302,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(3),
        }],
        ClientModel::Unique,
        None,
    )
}

#[test]
fn lean_settlement_sheds_the_per_round_history() {
    let mut state = compact_run_shape();
    state.drop_settled_spread = true;
    for _ in 1..=120 {
        state.draw_arrivals();
        let mask = vec![false; state.n];
        state.step_masked(&mask);
    }
    let settled: Vec<&CommandTracker> = state
        .trackers
        .iter()
        .filter(|t| t.committed_ack_round.is_some_and(|r| state.round > r + 1))
        .collect();
    let live: Vec<&CommandTracker> = state
        .trackers
        .iter()
        .filter(|t| t.committed_ack_round.is_none() && state.round >= t.injection_round)
        .collect();
    assert!(
        settled.len() > 20 && !live.is_empty(),
        "shape must yield both settled ({}) and live ({}) commands",
        settled.len(),
        live.len()
    );
    for t in &settled {
        assert!(
            t.attempts.is_empty() && t.amp_receivers.is_empty() && t.spread.is_empty(),
            "a settled tracker kept its history (attempts {}, receivers {}, spread {})",
            t.attempts.len(),
            t.amp_receivers.len(),
            t.spread.len()
        );
    }
    assert!(
        live.iter().all(|t| !t.attempts.is_empty()),
        "a live command must keep its attempts"
    );
}

#[test]
fn compact_keeps_a_cursor_after_the_safety_latch_drops() {
    let n = 64;
    let mut state = SmrState::new(
        n,
        Config::default(),
        Proto::Compact {
            t_commit_rounds: 21,
        },
        1.0,
        950_000,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(4),
        }],
        ClientModel::Unique,
        None,
    );
    let mask = vec![false; n];
    while state.safety_ok && state.round < 400 {
        state.draw_arrivals();
        state.step_masked(&mask);
    }
    assert!(!state.safety_ok, "seed 950000 must still split its brain");
    for _ in 0..5 {
        state.draw_arrivals();
        state.step_masked(&mask);
    }
    assert!(
        state.trackers.iter().any(|t| t.canon_cursor.is_some()
            && t.committed_ack_round.is_none_or(|r| state.round <= r + 1)),
        "a live compact tracker must hold a cursor once the latch has dropped"
    );
}

#[test]
fn recovery_unsafe_observer_enters_the_direct_scan_branch() {
    let mut state = recovery_state(false);
    let mask = vec![false; state.n];
    for _ in 0..5 {
        state.draw_arrivals();
        state.step_masked(&mask);
    }
    assert!(state.safety_ok, "the setup must begin in the stable branch");
    assert!(
        state.trackers.iter().any(|t| t.canon_cursor.is_some()),
        "the setup must open a recovery cursor"
    );

    state.safety_ok = false;
    state.draw_arrivals();
    state.step_masked(&mask);

    assert!(!state.safety_ok, "the unsafe branch must remain selected");
    assert!(
        state.trackers.iter().any(|t| t.canon_cursor.is_some()),
        "the unsafe observer keeps cursor lifetime and floor bookkeeping"
    );
}

#[test]
fn the_frontier_stays_monotone_and_below_every_live_cursor() {
    let n = 32;
    let mut state = SmrState::new(
        n,
        Config::default(),
        Proto::Compact {
            t_commit_rounds: 38,
        },
        1.0,
        975_301,
        &[],
        &[TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(4),
        }],
        ClientModel::Unique,
        None,
    );
    state.truncate_history = true;
    let mut last = 0;
    let mut advanced = 0;
    for round in 1..=150 {
        state.draw_arrivals();
        let mask = state.sample_mask(state.blocked_count(0.1));
        state.step_masked(&mask);
        assert!(
            state.frontier >= last,
            "round {round}: the frontier walked back to {}",
            state.frontier
        );
        if state.frontier > last {
            advanced += 1;
        }
        last = state.frontier;
        assert!(
            state.frontier <= state.checker.canonical_len() as u64,
            "round {round}: the frontier {} leads the canonical order {}",
            state.frontier,
            state.checker.canonical_len()
        );
        let live = state
            .trackers
            .iter()
            .filter_map(|t| t.canon_cursor.as_ref().map(|c| u64::from(c.floor())))
            .min();
        if let Some(floor) = live {
            assert!(
                state.frontier <= floor,
                "round {round}: the frontier {} passed a live cursor floor {floor}",
                state.frontier
            );
        }
    }
    assert!(
        advanced > 10,
        "the frontier barely moved ({advanced} rounds)"
    );
}

#[test]
fn the_retained_tail_is_flat_in_the_horizon() {
    let longest_tail = |state: &SmrState| {
        let lens = logical_lens(state);
        offsets(state)
            .iter()
            .zip(&lens)
            .map(|(offset, len)| len - offset)
            .max()
            .expect("nodes")
    };
    let committed = |state: &SmrState| logical_lens(state).into_iter().max().expect("nodes");

    let short = compact_run(true, 100, None);
    let long = compact_run(true, 300, None);
    assert!(
        committed(&long) > 2 * committed(&short),
        "the longer run did not commit more: {} vs {}",
        committed(&long),
        committed(&short)
    );
    assert!(
        longest_tail(&long) < 2 * longest_tail(&short),
        "the retained tail grew with the horizon: {} vs {}",
        longest_tail(&long),
        longest_tail(&short)
    );
}

/// Canonical entries, indexed first positions and double-execution positions still held.
fn observer_residency(state: &SmrState) -> [usize; 3] {
    let index = match &state.rec {
        Some(rec) => rec.canonical_pos_resident(),
        None => state.canon_index.resident(),
    };
    [
        state.checker.canonical().retained().len(),
        index,
        state.exec_dup.resident_positions(),
    ]
}

fn assert_observer_flat(short: &SmrState, long: &SmrState) {
    let (s, l) = (observer_residency(short), observer_residency(long));
    assert!(
        l[0] > 0 && l[1] > 0 && l[2] > 0,
        "the long run released everything, so nothing was measured: {l:?}"
    );
    for (what, (s, l)) in ["canonical", "entry index", "exec-dup positions"]
        .iter()
        .zip(s.iter().zip(&l))
    {
        assert!(
            *l < 2 * s,
            "the observer's {what} grew with the horizon: {l} vs {s}"
        );
    }
}

#[test]
fn the_compact_observer_is_flat_in_the_horizon() {
    assert_observer_flat(&compact_run(true, 100, None), &compact_run(true, 300, None));
}

#[test]
fn the_recovery_observer_is_flat_in_the_horizon() {
    assert_observer_flat(
        &recovery_run(true, 100, None),
        &recovery_run(true, 300, None),
    );
}

#[test]
fn pool_sequence_numbers_run_one_two_three_per_client() {
    let state = loaded_pool_state(48, 200);
    for (client, ts) in &by_client(&state) {
        let sns: Vec<u64> = ts.iter().map(|t| t.cc.sn).collect();
        assert_eq!(
            sns,
            (1..=ts.len() as u64).collect::<Vec<u64>>(),
            "client {client}: sequence numbers must be the client's own count"
        );
    }
}

#[test]
fn pool_sequence_numbers_match_injection_sns_on_the_same_script() {
    let state = loaded_pool_state(48, 200);
    by_client(&state);
    let script: Vec<Injection> = state
        .trackers
        .iter()
        .map(|t| Injection {
            round: t.injection_round,
            client: t.client - AUTO_CLIENT_BASE,
            op: t.op,
            target: None,
        })
        .collect();
    let mirrored = injection_sns(&script);
    let engine: Vec<u64> = state.trackers.iter().map(|t| t.cc.sn).collect();
    assert_eq!(engine, mirrored);
}

#[test]
fn a_pool_client_never_has_two_commands_in_flight() {
    let state = loaded_pool_state(48, 200);
    for (client, ts) in &by_client(&state) {
        for pair in ts.windows(2) {
            let (prev, next) = (pair[0], pair[1]);
            let acked = prev.committed_ack_round.unwrap_or_else(|| {
                panic!(
                    "client {client}: a successor minted while op {} was still in flight",
                    prev.op
                )
            });
            assert!(
                next.injection_round > acked,
                "client {client}: op {} minted in round {} before op {} was acked in round {acked}",
                next.op,
                next.injection_round,
                prev.op
            );
        }
    }
}

#[test]
fn the_grid_runner_does_not_observe_the_census() {
    let scenario = SmrScenario {
        n: 16,
        seed: 975_301,
        cfg: Config::default(),
        sigma: 1.0,
        proto: Proto::Compact { t_commit_rounds: 8 },
        injections: vec![],
        max_rounds: 60,
        schedule: crate::BlockSchedule::FreshPerRound { fraction: 0.0 },
        traffic: Some(vec![TrafficPhase {
            from_round: 1,
            arrivals_pmf: point_mass_pmf(2),
        }]),
        client_model: ClientModel::Unique,
        certs: false,
        manual_blocks: Vec::new(),
        partition: None,
        halt_on_violation: false,
        merge_policy: Default::default(),
        repeated_commit: Default::default(),
    };
    let grid = run_smr_grid(&scenario);
    let full = run_smr(&scenario);
    assert_eq!(
        grid.commands
            .iter()
            .all(|c| matches!(c.spread, SpreadCurve::NotObserved)),
        !grid_observes_census(),
        "the ledger records observes_census = {}, which is not what the \
             grid runner did",
        grid_observes_census()
    );
    assert!(
        full.commands
            .iter()
            .any(|c| c.spread.observed().is_some_and(|v| !v.is_empty())),
        "the full runner observed nothing, so the pairing pins nothing"
    );
}

#[test]
#[should_panic(
    expected = "spill boundary write failed for seed 77 (/full/disk/smr.jsonl): spill write: no space"
)]
fn a_spill_write_error_names_the_run_the_file_and_the_way_out() {
    spill_io_failure(
        "boundary write",
        77,
        Some(std::path::Path::new("/full/disk/smr.jsonl")),
        "spill write: no space",
    );
}

#[test]
#[should_panic(expected = "set SIM_SPILL=off")]
fn a_spill_write_error_says_how_to_run_without_the_stream() {
    spill_io_failure("create", 1, None, "no ledger directory");
}
