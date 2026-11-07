use super::*;
use crate::merge::sorted_perm;
use std::collections::BTreeMap;

fn null_reference(log: &mut [Timed]) {
    let mut ops: BTreeMap<(ClientId, u64), u64> = BTreeMap::new();
    let mut conflicting: BTreeSet<(ClientId, u64)> = BTreeSet::new();
    for t in log.iter() {
        if let Entry::Cmd(x) = t.entry
            && *ops.entry((x.client, x.sn)).or_insert(x.op) != x.op
        {
            conflicting.insert((x.client, x.sn));
        }
    }
    for t in log.iter_mut() {
        if let Entry::Cmd(x) = t.entry
            && conflicting.contains(&(x.client, x.sn))
        {
            t.entry = Entry::Null {
                client: x.client,
                sn: x.sn,
            };
        }
    }
}

// Conflict inside a sealed chunk: in the owned tail nothing shared needs protecting.
#[test]
fn nulling_through_a_shared_chunk_leaves_the_peer_untouched() {
    let clash = |op: u64| Timed {
        entry: Entry::Cmd(ClientCommand {
            client: 1,
            sn: 1,
            op,
        }),
        round: 0,
    };
    let filler = |i: u64| Timed {
        entry: Entry::Cmd(ClientCommand {
            client: 2,
            sn: i,
            op: i,
        }),
        round: 0,
    };

    let mut body = vec![clash(7), clash(8)];
    body.extend((0..(crate::chunked::CHUNK as u64 * 2 - 2)).map(filler));
    let donor = ChunkSeq::from_vec(body);
    assert!(
        donor.sealed_count() >= 2,
        "the clash must sit in a SEALED chunk"
    );

    let mut adopter = ChunkSeq::new();
    adopter.adopt(&donor);
    assert_eq!(
        donor.chunk_ptrs().collect::<Vec<_>>(),
        adopter.chunk_ptrs().collect::<Vec<_>>(),
        "the fixture needs the two to share allocations before the write"
    );

    let before: Vec<Timed> = donor.to_vec();
    let mut sorted: Vec<Entry> = adopter.iter().map(|t| t.entry.clone()).collect();
    sorted.sort_unstable();

    assert!(
        replace_duplicates_with_bot(&mut adopter, &sorted),
        "the fixture must actually trip the nulling pass"
    );

    assert_eq!(
        donor.to_vec(),
        before,
        "the peer's log was rewritten through the shared chunk"
    );
    let nulled = adopter
        .iter()
        .filter(|t| matches!(t.entry, Entry::Null { .. }))
        .count();
    assert_eq!(nulled, 2, "both conflicting commands null in the writer");
    assert_ne!(
        donor.chunk_ptrs().next(),
        adopter.chunk_ptrs().next(),
        "the written chunk must have been copied out of the sharing"
    );
    assert_eq!(
        donor.chunk_ptrs().nth(1),
        adopter.chunk_ptrs().nth(1),
        "chunks the write never touched stay shared"
    );
}

#[test]
fn adjacency_nulling_matches_the_two_map_reference() {
    let mut x: u64 = 7;
    let mut rand = move || {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        x >> 33
    };
    for case in 0..500 {
        let len = (rand() % 12) as usize;
        let log: Vec<Timed> = (0..len)
            .map(|_| {
                let e = match rand() % 5 {
                    0 => Entry::Null {
                        client: (rand() % 3) as ClientId,
                        sn: rand() % 3,
                    },
                    1 => Entry::Nop(rand() % 3),
                    _ => Entry::Cmd(ClientCommand {
                        client: (rand() % 3) as ClientId,
                        sn: rand() % 3,
                        op: rand() % 3,
                    }),
                };
                Timed {
                    entry: e,
                    round: rand() % 4,
                }
            })
            .collect();
        let mut sorted: Vec<Entry> = log.iter().map(|t| t.entry.clone()).collect();
        sorted.sort_unstable();
        let mut reference = log.clone();
        null_reference(&mut reference);
        let before = log.clone();
        let mut subject = ChunkSeq::from_vec(log.clone());
        let nulled = replace_duplicates_with_bot(&mut subject, &sorted);
        let log = subject.to_vec();
        assert_eq!(log, reference, "case {case}");
        assert_eq!(nulled, log != before, "case {case}: rewrite flag");
    }
}

struct Rand(u64);

impl Rand {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn gen_cmd(r: &mut Rand) -> ClientCommand {
    ClientCommand {
        client: r.below(3) as ClientId,
        sn: r.below(3),
        op: r.below(3),
    }
}

fn gen_timed(r: &mut Rand) -> Timed {
    let entry = match r.below(5) {
        0 => Entry::Null {
            client: r.below(3) as ClientId,
            sn: r.below(3),
        },
        1 => Entry::Nop(r.below(3)),
        _ => Entry::Cmd(gen_cmd(r)),
    };
    Timed {
        entry,
        round: r.below(4),
    }
}

// shape: 0 no prefix, 1 free, 2 all equal, 3 one a strict prefix, 4 entries agree but stamps differ.
fn gen_logs(r: &mut Rand, ell: usize, shape: u64) -> Vec<Vec<Timed>> {
    let p = match shape {
        0 => 0,
        4 => 1 + r.below(6) as usize,
        _ => r.below(7) as usize,
    };
    let prefix: Vec<Timed> = (0..p).map(|_| gen_timed(r)).collect();
    let shared_suffix: Vec<Timed> = (0..r.below(5)).map(|_| gen_timed(r)).collect();
    let mut logs: Vec<Vec<Timed>> = (0..ell)
        .map(|i| {
            let mut log = prefix.clone();
            match shape {
                2 => log.extend(shared_suffix.iter().cloned()),
                3 if i == 0 => {}
                _ => log.extend((0..r.below(6)).map(|_| gen_timed(r))),
            }
            log
        })
        .collect();
    if shape == 4 {
        let at = r.below(p as u64) as usize;
        for (i, log) in logs.iter_mut().enumerate() {
            log[at].round = 90 + i as u64;
        }
    }
    logs
}

fn common_prefix(logs: &[Vec<Timed>]) -> usize {
    let shortest = logs.iter().map(|l| l.len()).min().unwrap_or(0);
    (0..shortest)
        .take_while(|&i| logs[1..].iter().all(|l| l[i] == logs[0][i]))
        .count()
}

fn union_reference(
    replies: &[CompactReply],
    appends: &[(ClientCommand, u64)],
    ell: usize,
) -> (Vec<Timed>, Vec<Entry>, Vec<u32>) {
    let mut logs: Vec<&CompactReply> = replies.iter().collect();
    logs.sort_unstable_by(|a, b| a.l_j.entries().iter().cmp(b.l_j.entries().iter()));
    let median = logs[ell / 2];
    let mut log = median.l_j.entries().to_vec();
    let mut present: Vec<Entry> = median
        .l_j
        .perm()
        .iter()
        .map(|&i| log[i as usize].entry.clone())
        .collect();
    let mut perm_work: Vec<u32> = median.l_j.perm().as_ref().clone();
    let mut contains_or_insert = |entry: &Entry, place: u32| match present.binary_search(entry) {
        Ok(_) => false,
        Err(at) => {
            present.insert(at, entry.clone());
            perm_work.insert(at, place);
            true
        }
    };
    for t in logs.iter().flat_map(|l| l.l_j.entries().iter()) {
        if contains_or_insert(&t.entry, log.len() as u32) {
            log.push(t.clone());
        }
    }
    for (x, sent_round) in appends {
        let entry = Entry::Cmd(*x);
        if contains_or_insert(&entry, log.len() as u32) {
            log.push(Timed {
                entry,
                round: *sent_round,
            });
        }
    }
    (log, present, perm_work)
}

// Nothing ages out at round 0 with this delay, so `perm_work` is the carried index.
const NO_AGING: u64 = 1_000;
const MERGE_ROUND: u64 = 0;

fn step_reference(
    replies: &[CompactReply],
    appends: &[(ClientCommand, u64)],
    ell: usize,
) -> (Vec<Timed>, Vec<u32>, bool) {
    let (log, present, perm_work) = union_reference(replies, appends, ell);
    let mut subject = ChunkSeq::from_vec(log);
    let nulled = replace_duplicates_with_bot(&mut subject, &present);
    let mut perm = if nulled {
        sorted_perm(&subject)
    } else {
        perm_work
    };
    let mut log = subject.to_vec();
    if log.is_empty() {
        log.push(Timed {
            entry: Entry::Nop(MERGE_ROUND),
            round: MERGE_ROUND,
        });
        perm.push(0);
    }
    (log, perm, nulled)
}

fn step_merged(
    replies: &[CompactReply],
    appends: &[(ClientCommand, u64)],
    ell: usize,
) -> (Vec<Timed>, Vec<u32>) {
    let cfg = Config::new(ell, ell).expect("k = ell is a legal config for odd ell > 1");
    let mut node = CompactNode::new(cfg, NO_AGING);
    let chosen: Vec<usize> = (0..replies.len()).collect();
    node.step_chosen(replies, appends, MERGE_ROUND, Some(&chosen));
    let log = node.l_i.clone().expect("ell replies never go ⊥");
    (log.entries().to_vec(), log.perm().as_ref().clone())
}

#[test]
fn restricted_union_matches_the_full_walk_reference() {
    let mut r = Rand(0x5eed);
    let (mut with_prefix, mut mid_divergence, mut no_prefix, mut nullings) = (0, 0, 0, 0);
    for case in 0..600u64 {
        let ell = [3usize, 3, 5, 7][(case % 4) as usize];
        let shape = case % 5;
        let logs = gen_logs(&mut r, ell, shape);
        let replies: Vec<CompactReply> = logs
            .iter()
            .map(|l| CompactReply::from_log(Arc::new(ChunkSeq::from_vec(l.clone())), None))
            .collect();
        let appends: Vec<(ClientCommand, u64)> = (0..r.below(3))
            .map(|_| (gen_cmd(&mut r), r.below(4)))
            .collect();

        let (want_log, want_perm, nulled) = step_reference(&replies, &appends, ell);
        let (got_log, got_perm) = step_merged(&replies, &appends, ell);
        let label = format!("case {case} (ell {ell}, shape {shape})");
        assert_eq!(got_log, want_log, "{label}: merged log");
        assert_eq!(got_perm, want_perm, "{label}: carried index");

        let cp = common_prefix(&logs);
        let shortest = logs.iter().map(|l| l.len()).min().unwrap_or(0);
        with_prefix += usize::from(cp > 0);
        mid_divergence += usize::from(cp > 0 && cp < shortest);
        no_prefix += usize::from(cp == 0);
        nullings += usize::from(nulled);
    }
    // A corpus that never shares a prefix would pass vacuously.
    assert!(
        with_prefix >= 200,
        "only {with_prefix} cases share a prefix"
    );
    assert!(
        mid_divergence >= 100,
        "only {mid_divergence} cases diverge inside the shortest log"
    );
    assert!(no_prefix >= 100, "only {no_prefix} cases have cp = 0");
    assert!(nullings >= 20, "only {nullings} cases hit the nulling");
}

#[test]
fn a_conflict_straddling_the_common_prefix_still_nulls_both_sides() {
    let held = Entry::Cmd(ClientCommand {
        client: 1,
        sn: 1,
        op: 0,
    });
    let rival = Entry::Cmd(ClientCommand {
        client: 1,
        sn: 1,
        op: 1,
    });
    let prefix = vec![
        Timed {
            entry: held.clone(),
            round: 0,
        },
        Timed {
            entry: Entry::Nop(7),
            round: 0,
        },
    ];
    let suffixes = [
        rival.clone(),
        held.clone(),
        Entry::Cmd(ClientCommand {
            client: 2,
            sn: 2,
            op: 2,
        }),
    ];
    let logs: Vec<Vec<Timed>> = suffixes
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut log = prefix.clone();
            log.push(Timed {
                entry: e.clone(),
                round: 3 + i as u64,
            });
            log
        })
        .collect();
    assert_eq!(common_prefix(&logs), 2, "the fixture's shared prefix");

    let replies: Vec<CompactReply> = logs
        .iter()
        .map(|l| CompactReply::from_log(Arc::new(ChunkSeq::from_vec(l.clone())), None))
        .collect();
    let (want_log, want_perm, nulled) = step_reference(&replies, &[], 3);
    let (got_log, got_perm) = step_merged(&replies, &[], 3);
    assert!(nulled, "the fixture has to reach the nulling");
    assert_eq!(got_log, want_log);
    assert_eq!(got_perm, want_perm);

    let nulls = got_log
        .iter()
        .filter(|t| t.entry == Entry::Null { client: 1, sn: 1 })
        .count();
    assert_eq!(
        nulls, 2,
        "both sides of the straddling conflict: {got_log:?}"
    );
    assert!(
        !got_log.iter().any(|t| t.entry == held || t.entry == rival),
        "no surviving occurrence of the conflicting command: {got_log:?}"
    );
    assert_eq!(
        got_log.len(),
        4,
        "the re-offer must not append: {got_log:?}"
    );
}

/// Commits x through the ≥ ℓ path, then a re-spread copy of x through each path again.
fn commit_x_three_times(policy: RepeatedCommit) -> (Vec<Entry>, u64) {
    const T: u64 = 5;
    let x = ClientCommand {
        client: 7,
        sn: 1,
        op: 70,
    };
    let log_of = |stamp: u64| {
        CompactReply::from_log(
            Arc::new(ChunkSeq::from_vec(vec![Timed {
                entry: Entry::Cmd(x),
                round: stamp,
            }])),
            None,
        )
    };
    let mut node = CompactNode::new(Config::default(), T).with_repeated_commit(policy);
    node.step_chosen(&vec![log_of(1); 3], &[], 10, Some(&[0, 1, 2]));
    node.step_chosen(&vec![log_of(12); 3], &[], 20, Some(&[0, 1, 2]));
    node.step_chosen(&[log_of(22)], &[], 30, None);
    (node.s_i.executed().to_vec(), node.repeat_skips())
}

#[test]
fn a_recommitted_command_executes_again_by_default() {
    let (executed, skips) = commit_x_three_times(RepeatedCommit::Execute);
    assert_eq!(executed.len(), 3, "{executed:?}");
    assert_eq!(skips, 0);
}

#[test]
fn skip_executes_a_recommitted_command_once_on_both_commit_paths() {
    let (executed, skips) = commit_x_three_times(RepeatedCommit::Skip);
    assert_eq!(executed.len(), 1, "{executed:?}");
    assert_eq!(skips, 2, "one skip per path");
}

/// Three replies whose aged prefix commits client 7's sn 2 before its sn 1.
fn step_an_sn_gap(node: &mut CompactNode) {
    step_an_sn_gap_serving(node, None);
}

/// As [`step_an_sn_gap`], every reply also carrying `s_j`.
fn step_an_sn_gap_serving(node: &mut CompactNode, s_j: Option<Arc<SharedState>>) {
    let log = |entries: &[Entry]| {
        CompactReply::from_log(
            Arc::new(ChunkSeq::from_vec(
                entries
                    .iter()
                    .map(|e| Timed {
                        entry: e.clone(),
                        round: 0,
                    })
                    .collect(),
            )),
            s_j.clone(),
        )
    };
    let (x1, x2) = (
        ClientCommand {
            client: 7,
            sn: 1,
            op: 71,
        },
        ClientCommand {
            client: 7,
            sn: 2,
            op: 72,
        },
    );
    let replies = vec![log(&[Entry::Nop(0), Entry::Cmd(x2), Entry::Cmd(x1)]); 3];
    node.step_chosen(&replies, &[], 10, Some(&[0, 1, 2]));
}

#[test]
fn skip_refuses_an_out_of_sn_order_commit_and_keeps_s_and_l() {
    let mut node =
        CompactNode::new(Config::default(), 5).with_repeated_commit(RepeatedCommit::Skip);
    let log_before = node.log_entries();
    step_an_sn_gap(&mut node);
    assert_eq!(
        node.sn_order_violation(),
        Some(OutOfSnOrder {
            client: 7,
            sn: 2,
            committed_sn: 0
        })
    );
    assert!(node.s_i.executed().to_vec().is_empty(), "S_i is unchanged");
    assert_eq!(node.log_entries(), log_before, "L_i is unchanged");
    assert_eq!(node.repeat_skips(), 0);
}

// Box order (Alg 5, p. 26): "if b_i = 1 then i picks any reply S_j" precedes the commit, so a
// refused commit leaves S_j adopted and executes no entry of P on it.
#[test]
fn skip_refusal_at_b_i_1_keeps_the_adopted_s_j_and_executes_nothing() {
    let mut node =
        CompactNode::new(Config::default(), 5).with_repeated_commit(RepeatedCommit::Skip);
    node.step_chosen(&[], &[], 1, None);
    assert!(node.b_i(), "fixture: L_i = ⊥ sets b_i = 1");
    let s_j = Arc::new(SharedState::from_entries(
        vec![Entry::Cmd(ClientCommand {
            client: 9,
            sn: 1,
            op: 91,
        })],
        [(9, 1)].into(),
    ));
    step_an_sn_gap_serving(&mut node, Some(Arc::clone(&s_j)));
    assert_eq!(
        node.sn_order_violation(),
        Some(OutOfSnOrder {
            client: 7,
            sn: 2,
            committed_sn: 0
        })
    );
    assert!(
        Arc::ptr_eq(&node.s_i, &s_j),
        "S_j stays adopted, unexecuted"
    );
    assert_eq!(
        node.log_entries(),
        None,
        "L_i keeps its value from before the round"
    );
    assert_eq!(node.repeat_skips(), 0);
}

#[test]
fn execute_commits_the_same_gap_as_the_box_prints() {
    let mut node = CompactNode::new(Config::default(), 5);
    step_an_sn_gap(&mut node);
    assert_eq!(node.sn_order_violation(), None);
    assert_eq!(node.s_i.executed().to_vec().len(), 2);
}

#[test]
#[should_panic(expected = "refused an out-of-sn-order commit")]
fn a_node_that_refused_a_commit_is_not_stepped_again() {
    let mut node =
        CompactNode::new(Config::default(), 5).with_repeated_commit(RepeatedCommit::Skip);
    step_an_sn_gap(&mut node);
    node.step_chosen(&[], &[], 11, None);
}

// sn(c) = u64::MAX has no successor, so no sn(x) is "sn(c) + 1"; the exact match still acks.
#[test]
fn client_rule_at_the_largest_sequence_number_acks_and_never_amplifies() {
    let mut node = CompactNode::new(Config::default(), 5);
    node.s_i = Arc::new(SharedState::from_entries(
        Vec::new(),
        [(2, u64::MAX)].into(),
    ));
    let x = |sn| ClientCommand {
        client: 2,
        sn,
        op: 1,
    };
    assert_eq!(node.on_client_command(&x(u64::MAX)), Triage::AckCommitted);
    assert_eq!(node.on_client_command(&x(0)), Triage::Ignore);
}
