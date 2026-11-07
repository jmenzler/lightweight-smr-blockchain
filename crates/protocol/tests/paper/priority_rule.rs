use crate::law;
use protocol::{Config, PriorityNode, largest};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use rstest::rstest;

fn rng() -> ChaCha12Rng {
    ChaCha12Rng::seed_from_u64(42)
}

#[test]
fn largest_picks_the_maximum() {
    assert_eq!(largest(&[3, 9, 1]), 9);
}

#[test]
fn adopts_largest_of_the_ell_chosen_replies() {
    let mut n = PriorityNode::new(5, Config::default());
    // exactly ℓ replies: the chosen subset is all of them
    n.step(&[3, 9, 1], &mut rng());
    assert_eq!(n.x_i(), Some(9));
}

#[rstest]
#[case::six_three(6, 3, 2)]
#[case::seven_five(7, 5, 4)]
fn undecided_on_fewer_than_ell_replies(#[case] k: usize, #[case] ell: usize, #[case] got: usize) {
    let mut n = PriorityNode::new(5, Config::new(k, ell).unwrap());
    let replies = vec![9; got];
    n.step(&replies, &mut rng());
    assert_eq!(n.x_i(), None);
}

#[test]
fn adopted_value_is_always_one_of_the_replies() {
    let replies = [11, 5, 42, 7, 23, 5];
    for seed in 0..50u64 {
        let mut n = PriorityNode::new(1, Config::default());
        n.step(&replies, &mut ChaCha12Rng::seed_from_u64(seed));
        match n.x_i() {
            Some(v) => assert!(replies.contains(&v), "invented value {v}"),
            None => panic!("enough replies, must not go undecided"),
        }
    }
}

#[test]
fn adopted_value_follows_the_largest_of_a_uniform_ell_subset() {
    // Def 3.5: the largest of ℓ replies chosen uniformly at random, not of all replies.
    // Over the C(6,3) = 20 subsets of [11,5,42,7,23,5] the maximum is 42 in C(5,2) = 10,
    // 23 in C(4,2) = 6, 11 in C(3,2) = 3, 7 in C(2,2) = 1 and never 5. A max over all replies
    // always gives 42, and a content-chosen subset (the ℓ largest) does too.
    let replies = [11, 5, 42, 7, 23, 5];
    let adopted: Vec<_> = (0..law::TRIALS)
        .map(|seed| {
            let mut n = PriorityNode::new(1, Config::default());
            n.step(&replies, &mut ChaCha12Rng::seed_from_u64(seed));
            n.x_i().expect("at least ℓ replies")
        })
        .collect();
    law::assert_follows_law(
        &adopted,
        &[(5, 0.0), (7, 0.05), (11, 0.15), (23, 0.3), (42, 0.5)],
    );
}

#[test]
fn undecided_node_is_silent_and_recovers() {
    let mut n = PriorityNode::new(5, Config::default());
    n.step(&[], &mut rng());
    assert_eq!(n.answer(), None);
    n.step(&[4, 4, 8], &mut rng());
    assert_eq!(n.x_i(), Some(8));
}

#[test]
fn own_value_is_not_an_input_to_the_maximum() {
    // Definition 3.5: the largest of the chosen replies, whatever the own value was
    let mut n = PriorityNode::new(9, Config::default());
    n.step(&[3, 5, 1], &mut rng());
    assert_eq!(n.x_i(), Some(5));
}
