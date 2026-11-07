//! Algorithm 1, the (k,ℓ)-median rule (p. 10), and Algorithm 2, the (k,ℓ,f)-rule (p. 14). Each
//! bullet that runs here is marked `// Alg N: "<its opening words>"`, in the order of its box;
//! the bullet that only sends the k requests is the driver's.

use crate::{Config, Value};
use rand::Rng;
use rand::seq::index::sample;

/// Median of a non-empty value multiset.
pub fn median(values: &[Value]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// Server i under Algorithm 1 or 2.
pub struct MedianNode {
    /// x_i; `None` is ⊥.
    x_i: Option<Value>,
    cfg: Config,
}

impl MedianNode {
    /// "initially every server i stores an arbitrary value x_i ∈ K"
    pub fn new(x_i: Value, cfg: Config) -> Self {
        cfg.assert_valid();
        MedianNode {
            x_i: Some(x_i),
            cfg,
        }
    }

    /// A server that starts at ⊥ (the undecided state) and adopts a value on ≥ ℓ replies.
    pub fn new_undecided(cfg: Config) -> Self {
        cfg.assert_valid();
        MedianNode { x_i: None, cfg }
    }

    pub fn x_i(&self) -> Option<Value> {
        self.x_i
    }

    // Alg 1: "if x_i ≠ ⊥ then for any value request received from some server j"
    // Alg 2: "if x_i ≠ ⊥ then for any value request received from some server j"
    pub fn answer(&self) -> Option<Value> {
        self.x_i
    }

    // Alg 1: "if at least ℓ replies are received"
    // Alg 1: "if less than ℓ replies are received"
    /// One round of Algorithm 1: Algorithm 2 with f = median.
    pub fn step<R: Rng + ?Sized>(&mut self, replies: &[Value], rng: &mut R) {
        self.step_with(replies, rng, median);
    }

    /// One round of Algorithm 2. `f` must be order-invariant (not checked: it sees the ℓ replies
    /// in draw order) and return one of its inputs (checked, panics otherwise).
    pub fn step_with<R, F>(&mut self, replies: &[Value], rng: &mut R, f: F)
    where
        R: Rng + ?Sized,
        F: FnOnce(&[Value]) -> Value,
    {
        if replies.len() >= self.cfg.ell {
            // Alg 2: "if at least ℓ replies are received"
            let s: Vec<Value> = sample(rng, replies.len(), self.cfg.ell)
                .iter()
                .map(|j| replies[j])
                .collect();
            let x = f(&s);
            assert!(
                s.contains(&x),
                "f violated validity: {x} is not among its inputs"
            );
            self.x_i = Some(x);
        } else {
            // Alg 2: "if less than ℓ replies are received"
            self.x_i = None;
        }
    }
}
