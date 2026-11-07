use super::*;
use protocol::compact::ClientCommand;

fn state(op: u64) -> Arc<SharedState> {
    let cmd = Entry::Cmd(ClientCommand {
        client: 1,
        sn: 1,
        op,
    });
    Arc::new(SharedState::from_entries(vec![cmd], [(1, 1)].into()))
}

// Alg 5 L4d: a requester adopts S_j of the server that replied, so a reply must carry that server's state.
#[test]
fn a_reply_carries_the_targets_state_even_when_an_equal_length_state_was_served_first() {
    let (forked_a, forked_b) = (state(7), state(8));
    let mut by_len = BTreeMap::new();
    served_snapshot(&mut by_len, &forked_a);
    let served = served_snapshot(&mut by_len, &forked_b);
    assert!(
        served.content_eq(&forked_b),
        "reply carries another server's S_j"
    );
}

#[test]
fn equal_states_share_one_snapshot() {
    let (first, equal) = (state(7), state(7));
    let mut by_len = BTreeMap::new();
    let a = served_snapshot(&mut by_len, &first);
    let b = served_snapshot(&mut by_len, &equal);
    assert!(
        Arc::ptr_eq(&a, &b),
        "equal states should share one snapshot"
    );
}

fn compact_state(n: usize, t_commit_rounds: u64, seed: u64, rate: usize) -> SmrState {
    let mut arrivals_pmf = vec![0.0; rate + 1];
    arrivals_pmf[rate] = 1.0;
    SmrState::new(
        n,
        crate::Config::new(6, 3).expect("config"),
        crate::smr::Proto::Compact { t_commit_rounds },
        1.0,
        seed,
        &[],
        &[crate::smr::TrafficPhase {
            from_round: 1,
            arrivals_pmf,
        }],
        crate::smr::ClientModel::Unique,
        None,
    )
}

/// At the start of `round`: acked, its cursor closed (a round after the ack round + 1) and in every
/// log, so nothing is left for the live walk to do.
fn settled(t: &CommandTracker, round: usize) -> bool {
    t.committed_ack_round.is_some_and(|r| round > r + 2)
        && t.all_logs_round.is_some_and(|r| r < round)
}

// §1.2, p. 5 over a forked run (T = 4 is below the agreement time): commit dates still match a
// from-scratch recomputation once trackers leave the live set before being dated.
#[test]
fn commit_dates_match_a_recomputation_on_a_forked_run() {
    let mut state = compact_state(32, 4, 1, 4);
    let blocked = state.blocked_count(0.1);
    let mut want: BTreeMap<(u32, u64), usize> = BTreeMap::new();
    for _ in 0..60 {
        state.draw_arrivals();
        let status = state.step_sampled(blocked);
        let seqs = state.executed_seqs();
        let useful: Vec<usize> = state
            .compact_logs()
            .iter()
            .enumerate()
            .filter(|(i, log)| log.is_some() && !status.blocked.contains(&(*i as u32)))
            .map(|(i, _)| i)
            .collect();
        for t in &state.trackers {
            let entry = Entry::Cmd(t.cc);
            if !useful.is_empty() && useful.iter().all(|&i| seqs[i].contains(&entry)) {
                want.entry((t.cc.client, t.cc.sn)).or_insert(state.round);
            }
        }
    }
    assert!(!state.safety_ok, "precondition: the run forks");
    assert!(
        state
            .trackers
            .iter()
            .any(|t| settled(t, state.round) && t.committed_round.is_none()),
        "precondition: a settled tracker is never dated"
    );
    let got: BTreeMap<(u32, u64), usize> = state
        .trackers
        .iter()
        .filter_map(|t| t.committed_round.map(|r| ((t.cc.client, t.cc.sn), r)))
        .collect();
    assert_eq!(got, want);
}

// A settled tracker leaves the live set whether or not its commit is dated yet.
#[test]
fn the_live_set_starts_at_the_first_unsettled_tracker() {
    let mut state = compact_state(32, 8, 3, 1);
    let mut ahead_undated = 0;
    for _ in 0..120 {
        let round = state.round + 1;
        let first_unsettled = state
            .trackers
            .iter()
            .position(|t| !settled(t, round))
            .unwrap_or(state.trackers.len());
        ahead_undated += usize::from(
            state.trackers[..first_unsettled]
                .iter()
                .any(|t| t.committed_round.is_none()),
        );
        state.draw_arrivals();
        state.step_fraction(0.1);
        assert!(
            state.inert_prefix >= first_unsettled,
            "round {round}: the live set starts at {}, before the first unsettled tracker \
             {first_unsettled}",
            state.inert_prefix
        );
    }
    assert!(
        ahead_undated > 0,
        "precondition: a settled tracker waited for its commit date"
    );
}
