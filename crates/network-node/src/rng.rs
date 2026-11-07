//! Per-role RNG streams: ChaCha12 seeded from length-framed SHA-256(run_seed || role || id).
//!
//! Roles in use: "init" and "blocking" (shared by every node), "schedule", "traffic" and "client"
//! (generators and the client driver), "node" (a node's pull targets and its ℓ-subset draws, one
//! stream per node id), and per-command amplify streams, see [`derive_amplify_rng`].
//!
//! The simulator draws init, masks, targets and subsets from one shared stream, so the same seed
//! gives different values in the two tiers; they are compared by distribution, not trajectory.

use crate::NodeId;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;
use sha2::{Digest, Sha256};

pub fn derive_rng(run_seed: u64, role: &str, id: u32) -> ChaCha12Rng {
    let mut h = Sha256::new();
    h.update(run_seed.to_le_bytes());
    h.update((role.len() as u64).to_le_bytes());
    h.update(role.as_bytes());
    h.update(id.to_le_bytes());
    ChaCha12Rng::from_seed(h.finalize().into())
}

/// Amplify targets of one client command at one node: a stream of its own, keyed by the node and by
/// (round, client, sn) through the role string, so the draws never depend on which other commands
/// arrived or in what order, and never advance the node's pull-target stream.
pub fn derive_amplify_rng(
    run_seed: u64,
    node: NodeId,
    round: u64,
    client: u32,
    sn: u64,
) -> ChaCha12Rng {
    derive_rng(run_seed, &format!("amplify/{round}/{client}/{sn}"), node)
}

/// k pull targets uniform over `0..n` with replacement, own id included.
pub fn draw_targets(rng: &mut ChaCha12Rng, n: usize, k: usize) -> Vec<NodeId> {
    (0..k).map(|_| rng.random_range(0..n) as NodeId).collect()
}
