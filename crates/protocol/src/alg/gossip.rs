//! Algorithm 4, the (k,ℓ)-gossip rule (p. 21). Each bullet that runs here is marked
//! `// Alg 4: "<its opening words>"`, in the order of the box; the bullet that sends the k value
//! requests is the driver's.

use crate::{Config, Value};
use rand::Rng;

/// Server i under Algorithm 4.
pub struct GossipNode {
    /// x_i; `None` is ⊥.
    x_i: Option<Value>,
    /// The value being spread.
    x: Value,
    /// The dummy value x₀.
    x0: Value,
    cfg: Config,
}

impl GossipNode {
    /// "a subset of servers ... start with x_i = x, all others with x_i = x₀"
    pub fn new(x_i: Value, x: Value, x0: Value, cfg: Config) -> Self {
        cfg.assert_valid();
        assert!(x != x0, "gossip needs x ≠ x₀, got {x} twice");
        assert!(
            x_i == x || x_i == x0,
            "x_i must be x or x₀: got {x_i} with x = {x}, x₀ = {x0}"
        );
        GossipNode {
            x_i: Some(x_i),
            x,
            x0,
            cfg,
        }
    }

    pub fn x_i(&self) -> Option<Value> {
        self.x_i
    }

    // Alg 4: "if x_i ≠ ⊥ then for any value request received from some server j"
    pub fn answer(&self) -> Option<Value> {
        self.x_i
    }

    /// One round of Algorithm 4; `rng` is unused, kept for API symmetry.
    pub fn step<R: Rng + ?Sized>(&mut self, replies: &[Value], _rng: &mut R) {
        // Alg 4: "if at least one reply was received that contains x"
        self.x_i = if replies.contains(&self.x) {
            Some(self.x)
        } else {
            Some(self.x0)
        };
        // Alg 4: "if less than ℓ values are received"
        if replies.len() < self.cfg.ell {
            self.x_i = None;
        }
    }
}
