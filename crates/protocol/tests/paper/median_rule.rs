use crate::law;
use protocol::{Config, MedianNode, Value};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use rstest::rstest;

fn rng() -> ChaCha12Rng {
    ChaCha12Rng::seed_from_u64(42)
}

// --- Config preconditions (Algorithm 1 box: k, ℓ > 1, k ≥ ℓ, ℓ odd) ---

#[rstest]
#[case::k_too_small(1, 3)]
#[case::ell_too_small(6, 1)]
#[case::ell_zero(6, 0)]
#[case::ell_exceeds_k(3, 6)]
#[case::ell_even(6, 4)]
fn config_rejects_invalid_parameters(#[case] k: usize, #[case] ell: usize) {
    assert!(Config::new(k, ell).is_err());
}

#[rstest]
#[case(6, 3)]
#[case(3, 3)]
#[case(7, 5)]
fn config_accepts_valid_parameters(#[case] k: usize, #[case] ell: usize) {
    let cfg = Config::new(k, ell).unwrap();
    assert_eq!((cfg.k, cfg.ell), (k, ell));
}

#[test]
fn config_default_is_paper_instantiation() {
    let cfg = Config::default();
    assert_eq!((cfg.k, cfg.ell), (6, 3));
    assert!(Config::new(cfg.k, cfg.ell).is_ok());
}

// --- Algorithm 2: the (k,ℓ,f)-rule ---

#[rstest]
#[case::min(0)]
#[case::max(1)]
fn step_with_adopts_output_of_f(#[case] which: usize) {
    let fs: [fn(&[Value]) -> Value; 2] =
        [|s| *s.iter().min().unwrap(), |s| *s.iter().max().unwrap()];
    let expected = [1, 9];
    let mut node = MedianNode::new(7, Config::default());
    // exactly ℓ replies: the chosen subset is all of them
    node.step_with(&[3, 1, 9], &mut rng(), fs[which]);
    assert_eq!(node.x_i(), Some(expected[which]));
}

#[test]
fn step_with_passes_exactly_ell_values_to_f() {
    let mut node = MedianNode::new(7, Config::default());
    node.step_with(&[5, 6, 7, 8, 9, 10], &mut rng(), |s| {
        assert_eq!(s.len(), 3, "f must receive exactly ℓ values");
        s[0]
    });
}

#[test]
fn step_with_preserves_duplicates_as_multiset() {
    // paper box p.14: f(S) with S the MULTISET of values sent by the chosen replies
    let mut node = MedianNode::new(7, Config::default());
    node.step_with(&[4, 4, 4], &mut rng(), |s| {
        assert_eq!(s, [4, 4, 4], "duplicates must be preserved");
        s[0]
    });
    assert_eq!(node.x_i(), Some(4));
}

#[test]
fn step_with_goes_undecided_below_ell_regardless_of_f() {
    let mut node = MedianNode::new(7, Config::default());
    node.step_with(&[3, 9], &mut rng(), |_| {
        panic!("f must not be called below ℓ replies")
    });
    assert_eq!(node.x_i(), None);
}

/// Adopted value of `node` after one step on `replies`, for each of the seeds 0..TRIALS.
fn adopted_over_seeds(cfg: Config, own: Value, replies: &[Value]) -> Vec<Value> {
    (0..law::TRIALS)
        .map(|seed| {
            let mut node = MedianNode::new(own, cfg);
            node.step(replies, &mut ChaCha12Rng::seed_from_u64(seed));
            node.x_i().expect("at least ℓ replies")
        })
        .collect()
}

#[test]
fn adopted_value_follows_the_median_of_a_uniform_ell_subset() {
    // Alg 1: the median of ℓ replies chosen uniformly at random. Laws counted by hand over
    // the C(6,3) = 20 subsets; a max rule, a min rule, a median over all replies, or a
    // content-chosen subset each give a different law.
    let multiset = [5, 9, 9, 7, 5, 1]; // sorted 1,5,5,7,9,9: P(5) = 10/20, P(7) = 6/20, P(9) = 4/20
    law::assert_follows_law(
        &adopted_over_seeds(Config::default(), 100, &multiset),
        &[(1, 0.0), (5, 0.5), (7, 0.3), (9, 0.2)],
    );
    let distinct = [10, 20, 30, 40, 50, 60]; // rank j has (j-1)(6-j) of the 20 subsets as median
    law::assert_follows_law(
        &adopted_over_seeds(Config::default(), 100, &distinct),
        &[
            (10, 0.0),
            (20, 0.2),
            (30, 0.3),
            (40, 0.3),
            (50, 0.2),
            (60, 0.0),
        ],
    );
}

#[test]
fn median_index_is_right_for_ell_five() {
    // (7,5): the median of 5 values is the 3rd smallest. Of the C(7,5) = 21 subsets of seven
    // distinct values, rank j is the median in C(j-1,2)·C(7-j,2) = 6, 9, 6 for j = 3, 4, 5.
    let cfg = Config::new(7, 5).unwrap();
    law::assert_follows_law(
        &adopted_over_seeds(cfg, 100, &[1, 2, 3, 4, 5, 6, 7]),
        &[
            (1, 0.0),
            (2, 0.0),
            (3, 6.0 / 21.0),
            (4, 9.0 / 21.0),
            (5, 6.0 / 21.0),
            (6, 0.0),
            (7, 0.0),
        ],
    );
}

#[test]
fn own_value_does_not_enter_the_median() {
    // Alg 1 takes the median of the replies only: a server holding 100 among replies 1,1,1
    // adopts 1, and one holding 0 among 8,8,8 adopts 8.
    let mut high = MedianNode::new(100, Config::default());
    high.step(&[1, 1, 1], &mut rng());
    assert_eq!(high.x_i(), Some(1));
    let mut low = MedianNode::new(0, Config::default());
    low.step(&[8, 8, 8], &mut rng());
    assert_eq!(low.x_i(), Some(8));
}

#[test]
fn undecided_on_fewer_than_ell_replies() {
    let mut node = MedianNode::new(7, Config::default());
    node.step(&[3, 9], &mut rng());
    assert_eq!(node.x_i(), None);
}

#[test]
fn adopts_median_of_three_replies() {
    let mut node = MedianNode::new(7, Config::default());
    node.step(&[3, 1, 9], &mut rng());
    // exactly ell replies: the only 3-subset is all of them, median = 3
    assert_eq!(node.x_i(), Some(3));
}

#[test]
fn adopted_value_is_always_one_of_the_replies() {
    // validity: the median rule never invents a value
    let replies = [11, 5, 42, 7, 23, 5];
    for seed in 0..100u64 {
        let mut node = MedianNode::new(1, Config::default());
        let mut r = ChaCha12Rng::seed_from_u64(seed);
        node.step(&replies, &mut r);
        match node.x_i() {
            Some(v) => assert!(replies.contains(&v), "invented value {v}"),
            None => panic!("enough replies, must not go undecided"),
        }
    }
}

#[test]
fn new_undecided_node_starts_undecided_and_silent() {
    let node = MedianNode::new_undecided(Config::default());
    assert_eq!(node.x_i(), None);
    assert_eq!(node.answer(), None);
}

#[test]
fn undecided_node_does_not_reply() {
    let mut node = MedianNode::new(7, Config::default());
    node.step(&[], &mut rng()); // no replies -> undecided
    assert_eq!(node.answer(), None);
}

#[test]
fn deciding_node_replies_with_its_value() {
    let node = MedianNode::new(7, Config::default());
    assert_eq!(node.answer(), Some(7));
}

#[test]
fn undecided_node_recovers_on_enough_replies() {
    let mut node = MedianNode::new(7, Config::default());
    node.step(&[], &mut rng()); // -> undecided
    node.step(&[4, 4, 8], &mut rng()); // median of {4,4,8} = 4
    assert_eq!(node.x_i(), Some(4));
}

// --- Config preconditions hold on every node construction path, not only via Config::new ---

fn forged(k: usize, ell: usize) -> Config {
    Config { k, ell }
}

#[rstest]
#[case::ell_zero(6, 0)]
#[case::ell_one(6, 1)]
#[case::ell_even(6, 4)]
#[case::ell_exceeds_k(3, 5)]
#[case::k_one(1, 1)]
fn config_check_rejects_forged_parameters(#[case] k: usize, #[case] ell: usize) {
    assert!(forged(k, ell).check().is_err());
}

#[test]
fn config_check_accepts_the_paper_instance() {
    assert!(Config::default().check().is_ok());
}

#[rstest]
#[case::ell_zero(6, 0)]
#[case::ell_even(6, 4)]
#[case::ell_exceeds_k(3, 5)]
fn every_node_constructor_rejects_forged_config(#[case] k: usize, #[case] ell: usize) {
    use protocol::compact::CompactNode;
    use protocol::recovery::RecoveryNode;
    use protocol::{GossipNode, LogNode, PriorityNode};
    let cfg = forged(k, ell);
    let panics = |f: &dyn Fn()| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err();
    assert!(panics(&|| {
        let _ = MedianNode::new(1, cfg);
    }));
    assert!(panics(&|| {
        let _ = MedianNode::new_undecided(cfg);
    }));
    assert!(panics(&|| {
        let _ = PriorityNode::new(1, cfg);
    }));
    assert!(panics(&|| {
        let _ = GossipNode::new(1, 1, 0, cfg);
    }));
    assert!(panics(&|| {
        let _ = LogNode::new(1, cfg);
    }));
    assert!(panics(&|| {
        let _ = CompactNode::new(cfg, 4);
    }));
    assert!(panics(&|| {
        let _ = RecoveryNode::new(cfg, 4, false);
    }));
}

// --- Algorithm 2: f must return one of its inputs, in release builds too ---

#[test]
#[should_panic(expected = "validity")]
fn step_with_rejects_an_f_that_invents_a_value() {
    let mut node = MedianNode::new(7, Config::default());
    node.step_with(&[1, 2, 3], &mut rng(), |s| s.iter().sum());
}
