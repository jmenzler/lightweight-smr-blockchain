//! Liveness: the absorbing state in which no recovery node can ever hold a log again.

use protocol::recovery::RecoveryNode;

/// Every L_i = ⊥, so Alg 3 rebuilds nothing; every C_i has one W, so "If W' > W" adopts
/// nothing; and every node is stale (DEV-3 skips every boundary) or holds P = ⊥ (its rollback
/// restores ⊥). No step leaves this state, whatever the blocking or client traffic.
pub(in crate::smr) fn fleet_wedged(nodes: &[RecoveryNode]) -> bool {
    let Some(first) = nodes.first() else {
        return false;
    };
    let w = first.checkpoint().w;
    nodes.iter().all(|nd| {
        nd.log_seq().is_none()
            && nd.checkpoint().w == w
            && (nd.stale_checkpoint() || nd.checkpoint().p.is_none())
    })
}
