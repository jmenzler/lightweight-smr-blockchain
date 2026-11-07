//! Definition 3.5 (p. 23), the (k,ℓ)-priority rule: with at least ℓ replies a server chooses a
//! subset of ℓ of them and adopts the largest. The definition does not say how the subset is
//! chosen or whether the own value competes; the node runs Algorithm 2 with f = largest, so the
//! subset is uniform and the own value is excluded (README, "Gossip and priority readings").

use crate::median::MedianNode;
use crate::{Config, Value};
use rand::Rng;

/// Largest of a non-empty value multiset.
pub fn largest(values: &[Value]) -> Value {
    *values.iter().max().expect("non-empty by the ℓ-threshold")
}

pub struct PriorityNode {
    inner: MedianNode,
}

impl PriorityNode {
    pub fn new(x_i: Value, cfg: Config) -> Self {
        PriorityNode {
            inner: MedianNode::new(x_i, cfg),
        }
    }

    pub fn x_i(&self) -> Option<Value> {
        self.inner.x_i()
    }

    pub fn answer(&self) -> Option<Value> {
        self.inner.answer()
    }

    // Alg 2: "set x_i := f(S)", with f = largest (Definition 3.5)
    pub fn step<R: Rng + ?Sized>(&mut self, replies: &[Value], rng: &mut R) {
        self.inner.step_with(replies, rng, largest);
    }
}
