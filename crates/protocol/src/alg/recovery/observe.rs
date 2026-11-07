//! Read-only views a driver or observer takes of a recovery node; no box step runs here.

use super::{Checkpoint, RState, RecoveryNode, RecoveryPrefixViolation};
use crate::chunked::ChunkSeq;
use crate::compact::{SharedState, Timed};
use crate::log::Log;
use crate::support::checkpoint::{aged_prefix, validate_prefix};
use std::sync::Arc;

impl RecoveryNode {
    pub fn log_seq(&self) -> Option<&ChunkSeq<Timed>> {
        self.l_i.as_ref().map(|l| &**l.entries())
    }

    pub fn log_entries(&self) -> Option<Vec<Timed>> {
        self.log_seq().map(ChunkSeq::to_vec)
    }

    pub fn log_arc(&self) -> Option<&Arc<ChunkSeq<Timed>>> {
        self.l_i.as_ref().map(Log::entries)
    }

    pub fn log_perm_arc(&self) -> Option<&Log> {
        self.l_i.as_ref()
    }

    pub fn shared_state(&self) -> &SharedState {
        &self.s_i
    }

    pub fn state_arc(&self) -> &Arc<SharedState> {
        &self.s_i
    }

    pub fn reset_state(&self) -> RState {
        self.r_i
    }

    pub fn repeat_skips(&self) -> u64 {
        self.repeat_skips
    }

    /// True from a DEV-3 boundary skip until adoption of a newer peer checkpoint.
    pub fn stale_checkpoint(&self) -> bool {
        self.stale_checkpoint
    }

    pub fn checkpoint(&self) -> &Checkpoint {
        &self.c_i
    }

    pub fn checkpoint_shared(&self) -> Arc<Checkpoint> {
        Arc::clone(&self.c_i)
    }

    /// Whether the boundary's "P is a prefix of L_i" holds for this node alone. A rollback sets
    /// L_i := P first, so a reset node passes by construction: whether its P agrees with the
    /// other servers' same-W checkpoints (Lemma 6.7) is a population question this cannot see.
    pub fn validate_boundary(&self) -> Result<(), RecoveryPrefixViolation> {
        if self.r_i == RState::Reset {
            return Ok(());
        }
        let Some(log) = self.log_seq() else {
            return Ok(());
        };
        let pre = self.c_i.p.as_deref().unwrap_or(&[]);
        validate_prefix(pre, log.len(), log.iter())
    }

    /// The P this node's next checkpoint would carry if the boundary ran now.
    pub fn would_carry_pre(&self, round: u64) -> Option<Vec<Timed>> {
        if self.stale_checkpoint {
            return None;
        }
        self.validate_boundary().ok()?;
        let log: Vec<Timed> = if self.r_i == RState::Reset {
            self.c_i.p.clone()?
        } else {
            self.log_seq()?.to_vec()
        };
        // validate_boundary() proved `log` starts with the committed P.
        let committed = self.c_i.p.as_ref().map_or(0, Vec::len);
        // A refilled `Nop(round)` is never aged, so empty and refilled logs both carry an empty P.
        Some(aged_prefix(&log[committed..], round, self.t))
    }
}
