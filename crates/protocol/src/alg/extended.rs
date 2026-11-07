//! Algorithm 3, the extended (k,ℓ)-median rule (p. 20). Each bullet is marked
//! `// Alg 3: "<its opening words>"`, in the order of the box.

use crate::Config;
use rand::Rng;
use rand::seq::index::sample;
use std::sync::Arc;

pub type Command = u64;

/// Server i under Algorithm 3.
pub struct LogNode {
    /// L_i; `None` is ⊥. Shared so that a reply hands out the log without copying it.
    l_i: Option<Arc<Vec<Command>>>,
    cfg: Config,
}

impl LogNode {
    /// "Initially, L_i = (x₀) for all i"
    pub fn new(x0: Command, cfg: Config) -> Self {
        cfg.assert_valid();
        LogNode {
            l_i: Some(Arc::new(vec![x0])),
            cfg,
        }
    }

    pub fn log(&self) -> Option<&[Command]> {
        self.l_i.as_deref().map(Vec::as_slice)
    }

    // Alg 3: "for every command x received from a client that is not yet contained in L_i"
    pub fn wants_amplify(&self, x: Command) -> bool {
        match &self.l_i {
            None => true,
            Some(l_i) => !l_i.contains(&x),
        }
    }

    // Alg 3: "if L_i ≠ ⊥ then for any log request received by server i from a server j"
    pub fn answer(&self) -> Option<Arc<Vec<Command>>> {
        self.l_i.clone()
    }

    /// One round; `replies` are the logs L_j received, `appends` the append requests.
    pub fn step<R: Rng + ?Sized>(
        &mut self,
        replies: &[Arc<Vec<Command>>],
        appends: &[Command],
        rng: &mut R,
    ) {
        if replies.len() >= self.cfg.ell {
            // Alg 3: "if server i receives at least ℓ replies"
            // Alg 3: "server i chooses a subset M of size ℓ from the received logs"
            let mut m: Vec<&Arc<Vec<Command>>> = sample(rng, replies.len(), self.cfg.ell)
                .iter()
                .map(|j| &replies[j])
                .collect();
            // Alg 3: "server i sets L_i := L'_i ∘ L̄"
            // DETERMINIZED: [DET-12] L̄ is the sorted M in order, then the appends in the driver's order
            m.sort_unstable_by(|a, b| a[..].cmp(&b[..]));
            let l_prime = m[self.cfg.ell / 2];
            let mut l_i = l_prime.as_ref().clone();
            for &x in m.iter().flat_map(|l| l.iter()).chain(appends) {
                if !l_i.contains(&x) {
                    l_i.push(x);
                }
            }
            debug_assert!(l_i[..l_prime.len()] == l_prime[..]);
            self.l_i = Some(Arc::new(l_i));
        } else {
            // Alg 3: "if server i receives less than ℓ logs"
            self.l_i = None;
        }
    }
}
