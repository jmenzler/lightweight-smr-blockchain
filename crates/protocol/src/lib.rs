//! The (k,ℓ)-median-rule protocol family from arXiv:2509.17771v2, one module per rule.

mod alg;
pub mod certificates;
mod support;
#[cfg(test)]
mod tag_map;

pub use alg::{compact, extended, gossip, median, priority, recovery};
pub use support::{chunked, log, merge, shared_state};

pub use extended::{Command, LogNode};
pub use gossip::GossipNode;
pub use median::{MedianNode, median};
pub use priority::{PriorityNode, largest};

pub type Value = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Config {
    pub k: usize,
    pub ell: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidConfig;

impl std::fmt::Display for InvalidConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "median-rule config requires k, ell > 1, k >= ell, ell odd"
        )
    }
}

impl std::error::Error for InvalidConfig {}

impl Config {
    /// Preconditions of Algorithm 1: k, ℓ > 1, k ≥ ℓ, ℓ odd.
    pub fn new(k: usize, ell: usize) -> Result<Self, InvalidConfig> {
        Config { k, ell }.check()
    }

    /// Re-checks the preconditions, which the public fields let a caller bypass.
    pub fn check(self) -> Result<Self, InvalidConfig> {
        if self.k > 1 && self.ell > 1 && self.k >= self.ell && self.ell % 2 == 1 {
            Ok(self)
        } else {
            Err(InvalidConfig)
        }
    }

    /// Node constructors call this so a forged config fails at construction.
    pub(crate) fn assert_valid(self) {
        if let Err(e) = self.check() {
            panic!("{e}: got k = {}, ell = {}", self.k, self.ell);
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config { k: 6, ell: 3 }
    }
}
