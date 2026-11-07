use crate::law;
use protocol::{Command, Config, LogNode};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use rstest::rstest;

/// Replies reach the rule as shared handles; the tests keep the plain
/// vectors so their assertions still read the content directly.
fn shared(logs: &[Vec<Command>]) -> Vec<std::sync::Arc<Vec<Command>>> {
    logs.iter().cloned().map(std::sync::Arc::new).collect()
}

fn rng() -> ChaCha12Rng {
    ChaCha12Rng::seed_from_u64(42)
}

fn node() -> LogNode {
    LogNode::new(0, Config::default())
}

#[test]
fn starts_with_seed_command_log() {
    assert_eq!(node().log(), Some(&[0][..]));
}

#[rstest]
#[case::six_three(6, 3, 2)]
#[case::seven_five(7, 5, 4)]
fn undecided_on_fewer_than_ell_replies(#[case] k: usize, #[case] ell: usize, #[case] got: usize) {
    let mut n = LogNode::new(0, Config::new(k, ell).unwrap());
    let replies = vec![vec![0, 1]; got];
    n.step(&shared(&replies), &[], &mut rng());
    assert_eq!(n.log(), None, "below ℓ replies the log must be ⊥");
}

#[test]
fn undecided_node_does_not_reply() {
    // Alg 3: a server with L_i = ⊥ sends no log back; a server with L_i ≠ ⊥ sends it
    let mut n = node();
    assert_eq!(n.answer().as_deref(), Some(&vec![0]));
    n.step(&[], &[], &mut rng());
    assert_eq!(n.log(), None);
    assert!(n.answer().is_none());
    n.step(&shared(&vec![vec![0, 4]; 3]), &[], &mut rng());
    assert_eq!(n.answer().as_deref(), Some(&vec![0, 4]));
}

#[test]
fn adopts_lexicographic_median_log_and_appends_missing() {
    let mut n = node();
    // lex order: [0,1,2] < [0,1,3] < [0,2,1] → median [0,1,3]; missing: {2}
    let replies = vec![vec![0, 1, 2], vec![0, 1, 3], vec![0, 2, 1]];
    n.step(&shared(&replies), &[], &mut rng());
    assert_eq!(n.log(), Some(&[0, 1, 3, 2][..]));
}

#[test]
fn append_requests_join_the_log_tail() {
    let mut n = node();
    n.step(
        &[
            std::sync::Arc::new(vec![0]),
            std::sync::Arc::new(vec![0]),
            std::sync::Arc::new(vec![0]),
        ],
        &[5],
        &mut rng(),
    );
    assert_eq!(n.log(), Some(&[0, 5][..]));
}

#[test]
fn append_request_already_in_median_is_not_duplicated() {
    let mut n = node();
    n.step(
        &[
            std::sync::Arc::new(vec![0, 5]),
            std::sync::Arc::new(vec![0, 5]),
            std::sync::Arc::new(vec![0, 5]),
        ],
        &[5],
        &mut rng(),
    );
    assert_eq!(n.log(), Some(&[0, 5][..]));
}

#[test]
fn merged_log_never_repeats_a_command() {
    // K* = sequences without repetitions; L̄ must dedup across sources
    let mut n = node();
    let replies = vec![vec![0, 7], vec![0, 8], vec![0, 9]];
    n.step(&shared(&replies), &[7, 8, 9], &mut rng());
    let log = n.log().unwrap();
    let mut seen = std::collections::HashSet::new();
    assert!(
        log.iter().all(|c| seen.insert(*c)),
        "repeated command in {log:?}"
    );
}

#[test]
fn adopted_commands_all_come_from_replies_or_appends() {
    // validity: the rule never invents commands
    for seed in 0..50u64 {
        let mut n = node();
        let replies = vec![vec![0, 1], vec![0, 2], vec![0, 3], vec![0, 1, 4]];
        let appends = [9];
        let mut r = ChaCha12Rng::seed_from_u64(seed);
        n.step(&shared(&replies), &appends, &mut r);
        let known: std::collections::HashSet<Command> =
            replies.iter().flatten().copied().chain(appends).collect();
        for c in n.log().unwrap() {
            assert!(known.contains(c), "invented command {c}");
        }
    }
}

#[test]
fn median_log_stays_a_prefix_of_the_result() {
    // L_i := L'_i ∘ L̄ — the median log is never reordered, only extended
    for seed in 0..50u64 {
        let mut n = node();
        let replies = vec![vec![0, 1, 2], vec![0, 1, 3], vec![0, 2, 1]];
        let mut r = ChaCha12Rng::seed_from_u64(seed);
        n.step(&shared(&replies), &[], &mut r);
        assert_eq!(&n.log().unwrap()[..3], &[0, 1, 3]);
    }
}

#[rstest]
#[case::fresh_command(5, true)]
#[case::already_logged(0, false)]
fn wants_amplify_only_new_commands(#[case] x: Command, #[case] expected: bool) {
    assert_eq!(node().wants_amplify(x), expected);
}

#[test]
fn undecided_node_amplifies_unseen_commands() {
    let mut n = node();
    n.step(&[], &[], &mut rng());
    assert!(
        n.wants_amplify(5),
        "Algorithm 3 requires an unblocked ⊥ node to send append requests"
    );
}

#[test]
fn undecided_node_recovers_on_enough_replies() {
    let mut n = node();
    n.step(&[], &[], &mut rng());
    n.step(
        &[
            std::sync::Arc::new(vec![0, 4]),
            std::sync::Arc::new(vec![0, 4]),
            std::sync::Arc::new(vec![0, 4]),
        ],
        &[],
        &mut rng(),
    );
    assert_eq!(n.log(), Some(&[0, 4][..]));
}

#[test]
fn m_leaves_out_each_reply_instance_equally_often() {
    // Each reply holds one private command, so the command missing from the result names the reply left out of M.
    let replies = shared(&[vec![0, 1], vec![0, 2], vec![0, 3], vec![0, 4]]);
    let mut left_out = [0u32; 4];
    for seed in 0..4000u64 {
        let mut n = node();
        n.step(&replies, &[], &mut ChaCha12Rng::seed_from_u64(seed));
        let log = n.log().unwrap();
        let missing = (1..=4)
            .find(|c| !log.contains(c))
            .expect("ℓ = 3 of 4 replies leaves one out");
        left_out[missing as usize - 1] += 1;
    }
    // 1000 each in expectation, sigma about 27.
    assert!(
        left_out.iter().all(|c| (880..=1120).contains(c)),
        "{left_out:?}"
    );
}

#[test]
fn identical_logs_are_separate_reply_instances_of_the_draw() {
    // All three copies of A drawn together have probability 1 / C(5,3); a draw over distinct logs would never give it.
    let a = vec![0, 1];
    let replies = shared(&[a.clone(), a.clone(), a.clone(), vec![0, 2], vec![0, 3]]);
    let only_a = (0..5000u64)
        .filter(|&seed| {
            let mut n = node();
            n.step(&replies, &[], &mut ChaCha12Rng::seed_from_u64(seed));
            n.log() == Some(&a[..])
        })
        .count();
    // 500 in expectation, sigma about 21.
    assert!((400..=600).contains(&only_a), "{only_a}");
}

#[test]
fn append_requests_are_lost_with_the_log_below_ell_replies() {
    let mut n = node();
    n.step(&shared(&[vec![0], vec![0]]), &[7], &mut rng());
    assert_eq!(n.log(), None);
    n.step(&shared(&vec![vec![0]; 3]), &[], &mut rng());
    assert_eq!(
        n.log(),
        Some(&[0][..]),
        "the lost append must not resurface"
    );
}

#[test]
fn a_command_held_only_by_the_server_itself_is_dropped() {
    // L̄ draws on the chosen replies and the appends; the server's own previous log counts only if it was drawn.
    let mut n = node();
    n.step(&shared(&vec![vec![0]; 3]), &[5], &mut rng());
    assert_eq!(n.log(), Some(&[0, 5][..]));
    n.step(&shared(&vec![vec![0]; 3]), &[], &mut rng());
    assert_eq!(n.log(), Some(&[0][..]));
}

// --- lexicographic order and the median index (Alg 3 P2: true dictionary order) ---

/// One step of a fresh node with ℓ replies, so that the subset M is all of them.
fn step_on_exactly_ell(cfg: Config, replies: &[Vec<Command>]) -> Vec<Command> {
    let mut n = LogNode::new(0, cfg);
    n.step(&shared(replies), &[], &mut rng());
    n.log().unwrap().to_vec()
}

fn assert_median_prefix_then_rest(log: &[Command], median: &[Command], rest: &[Command]) {
    assert_eq!(&log[..median.len()], median, "median prefix of {log:?}");
    let mut tail = log[median.len()..].to_vec();
    tail.sort_unstable();
    let mut expected = rest.to_vec();
    expected.sort_unstable();
    assert_eq!(tail, expected, "commands after the median in {log:?}");
}

#[test]
fn median_orders_by_first_difference_not_by_length() {
    // dictionary order: [0,1,5,6] < [0,2] < [0,3], median [0,2]. A length-first order would
    // sort [0,2] < [0,3] < [0,1,5,6] and pick [0,3].
    let log = step_on_exactly_ell(
        Config::default(),
        &[vec![0, 1, 5, 6], vec![0, 2], vec![0, 3]],
    );
    assert_median_prefix_then_rest(&log, &[0, 2], &[1, 5, 6, 3]);
}

#[rstest]
#[case::prefix_listed_first(vec![vec![0, 1], vec![0, 1, 2], vec![0, 2]])]
#[case::extension_listed_first(vec![vec![0, 1, 2], vec![0, 1], vec![0, 2]])]
fn a_proper_prefix_is_smaller_than_its_extension(#[case] replies: Vec<Vec<Command>>) {
    // [0,1] < [0,1,2] < [0,2] in dictionary order; the median is [0,1,2]. Length-first order
    // would pick [0,2], and ranking an extension below its prefix would pick [0,1].
    let log = step_on_exactly_ell(Config::default(), &replies);
    assert_eq!(log, [0, 1, 2]);
}

#[test]
fn median_of_five_is_the_third_smallest_log() {
    // (7,5): the ((ℓ+1)/2) = 3rd smallest of five logs. Dictionary order:
    // [0,1,7] < [0,3] < [0,5] < [0,5,1] < [0,9], median [0,5]. Length-first order would pick [0,9].
    let cfg = Config::new(7, 5).unwrap();
    let log = step_on_exactly_ell(
        cfg,
        &[
            vec![0, 9],
            vec![0, 3],
            vec![0, 5, 1],
            vec![0, 5],
            vec![0, 1, 7],
        ],
    );
    assert_median_prefix_then_rest(&log, &[0, 5], &[1, 7, 3, 9]);
}

// --- L̄ is built from the logs in M and this round's appends only ---

#[test]
fn own_log_does_not_feed_the_merge() {
    // Alg 3 L̄: "all values in logs in M or in append requests ... that are not in L'_i". A
    // server whose own log is [0,5] and whose replies are all [0] ends with [0].
    let mut n = node();
    n.step(&shared(&vec![vec![0, 5]; 3]), &[], &mut rng());
    assert_eq!(n.log(), Some(&[0, 5][..]));
    n.step(&shared(&vec![vec![0]; 3]), &[], &mut rng());
    assert_eq!(n.log(), Some(&[0][..]));
}

#[test]
fn only_the_logs_in_the_chosen_subset_are_merged() {
    // Four replies with ℓ = 3: M leaves one reply out. Each reply carries a command no other
    // reply has, so the result holds the median prefix and exactly the two other commands of
    // M, 4 elements, never the 5 of an all-replies merge; each command is present with the
    // probability 3/4 that its reply is in the uniform 3-subset.
    let replies = vec![vec![0, 1], vec![0, 2], vec![0, 3], vec![0, 4]];
    let mut present = [0u64; 5];
    for seed in 0..law::TRIALS {
        let mut n = node();
        n.step(
            &shared(&replies),
            &[],
            &mut ChaCha12Rng::seed_from_u64(seed),
        );
        let log = n.log().unwrap();
        assert_eq!(log.len(), 4, "seed {seed}: {log:?}");
        for c in 1..=4 {
            present[c as usize] += u64::from(log.contains(&c));
        }
    }
    for (c, &hits) in present.iter().enumerate().skip(1) {
        law::assert_frequency(&format!("command {c} merged"), hits, law::TRIALS, 0.75);
    }
}
