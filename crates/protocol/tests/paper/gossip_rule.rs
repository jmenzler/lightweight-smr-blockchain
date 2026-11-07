use protocol::{Config, GossipNode};
use rand::SeedableRng;
use rand_chacha::ChaCha12Rng;
use rstest::rstest;

const X: u64 = 7; // the value being spread
const X0: u64 = 0; // the dummy value

fn rng() -> ChaCha12Rng {
    ChaCha12Rng::seed_from_u64(42)
}

fn holder() -> GossipNode {
    GossipNode::new(X, X, X0, Config::default())
}

fn non_holder() -> GossipNode {
    GossipNode::new(X0, X, X0, Config::default())
}

#[test]
fn nodes_reply_with_their_current_value() {
    assert_eq!(holder().answer(), Some(X));
    assert_eq!(non_holder().answer(), Some(X0));
}

#[rstest]
fn adopts_x_when_any_reply_contains_it(#[values(0, 1, 2, 3, 4, 5)] position: usize) {
    // Alg 4: one reply carrying x among six suffices, at whichever position it arrives and
    // whatever the generator draws: a gossip server takes no ℓ-subset of its replies.
    let mut replies = [X0; 6];
    replies[position] = X;
    for seed in 0..200u64 {
        let mut n = non_holder();
        n.step(&replies, &mut ChaCha12Rng::seed_from_u64(seed));
        assert_eq!(n.x_i(), Some(X), "x at position {position}, seed {seed}");
    }
}

#[test]
fn falls_back_to_dummy_when_no_reply_contains_x() {
    // a holder LOSES x if none of its sampled peers has it
    let mut n = holder();
    n.step(&[X0, X0, X0], &mut rng());
    assert_eq!(n.x_i(), Some(X0));
}

#[rstest]
#[case::six_three(6, 3, 2)]
#[case::seven_five(7, 5, 4)]
fn undecided_on_fewer_than_ell_replies(#[case] k: usize, #[case] ell: usize, #[case] got: usize) {
    let mut n = GossipNode::new(X, X, X0, Config::new(k, ell).unwrap());
    let replies = vec![X; got];
    n.step(&replies, &mut rng());
    assert_eq!(n.x_i(), None, "below ℓ replies the node must be ⊥");
}

#[test]
fn undecided_node_is_silent_and_recovers() {
    let mut n = holder();
    n.step(&[], &mut rng());
    assert_eq!(n.answer(), None);
    n.step(&[X0, X0, X], &mut rng());
    assert_eq!(n.x_i(), Some(X));
}

#[test]
fn below_ell_wins_even_if_x_present() {
    // step 4 overrides step 3: too few replies → ⊥ even when x was seen
    let mut n = non_holder();
    n.step(&[X, X], &mut rng());
    assert_eq!(n.x_i(), None);
}

#[rstest]
#[case::own_value_outside_x_and_x0(99, X, X0)]
#[case::x_equals_x0(X, X, X)]
fn constructor_rejects_values_outside_the_alg4_state_space(
    #[case] x_i: u64,
    #[case] x: u64,
    #[case] x0: u64,
) {
    let built = std::panic::catch_unwind(|| GossipNode::new(x_i, x, x0, Config::default()));
    assert!(built.is_err());
}
