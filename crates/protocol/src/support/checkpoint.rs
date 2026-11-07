//! The log and state operations Algorithm 6 names on a checkpoint's prefix P:
//! L_i := P, execute P on S_i, remove P from L_i, and the aged prefix P_i.

use crate::chunked::ChunkSeq;
use crate::compact::{Entry, Timed, aged_prefix_len, x_d};
use crate::log::Log;
use crate::support::shared_state::{
    CLONE_SITE_RECOVERY_COW_EXECUTE, OutOfSnOrder, RepeatedCommit, SharedState, cow_state,
};
use std::sync::Arc;

/// P's commands are not a prefix of L_i's at a window boundary, which the paper rules out
/// w.h.p. (Lemma 6.7). `first_mismatch` is the first index whose command differs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RecoveryPrefixViolation {
    pub pre_len: usize,
    pub log_len: usize,
    pub first_mismatch: usize,
    pub expected: Option<Timed>,
    pub actual: Option<Timed>,
}

impl std::fmt::Display for RecoveryPrefixViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "P must be a prefix of L: strip would corrupt the log (pre len {}, log len {}, first mismatch {}, expected {:?}, actual {:?})",
            self.pre_len, self.log_len, self.first_mismatch, self.expected, self.actual,
        )
    }
}

impl std::error::Error for RecoveryPrefixViolation {}

/// P prefixes L_i when its commands are L_i's first |P| commands. Alg 6 (p. 31) defines P as a
/// "sequence of pre-committed commands"; the round stamps are age bookkeeping and take no part.
// DETERMINIZED: [DET-15] the prefix test compares commands only, and the strip drops L_i's first len(P) entries
pub(crate) fn validate_prefix<'a>(
    pre: &[Timed],
    log_len: usize,
    mut log: impl Iterator<Item = &'a Timed>,
) -> Result<(), RecoveryPrefixViolation> {
    for (index, expected) in pre.iter().enumerate() {
        let actual = log.next();
        if actual.map(|t| &t.entry) != Some(&expected.entry) {
            return Err(RecoveryPrefixViolation {
                pre_len: pre.len(),
                log_len,
                first_mismatch: index,
                expected: Some(expected.clone()),
                actual: actual.cloned(),
            });
        }
    }
    Ok(())
}

/// Panics unless P is a prefix of L_i: a violation must abort visibly, not corrupt the log.
pub(crate) fn assert_prefix(p: &[Timed], log: &ChunkSeq<Timed>, round: u64) {
    if let Err(violation) = validate_prefix(p, log.len(), log.iter()) {
        panic!("{violation} at round {round}");
    }
}

/// L_i := P with its carried index rebuilt; P = ⊥ gives L_i = ⊥.
// DETERMINIZED: [DET-14] a rollback to the genesis checkpoint (P = ⊥) leaves L_i = ⊥, not an empty valid log
pub(crate) fn log_from_prefix(p: Option<&[Timed]>) -> Option<Log> {
    p.map(|p| Log::from_entries(p.to_vec()))
}

/// Executes P on S_i and returns how many entries the execute-once state machine left out.
/// A P of no-ops only leaves S_i's allocation untouched; a refused P leaves S_i unchanged.
pub(crate) fn execute_prefix(
    state: &mut Arc<SharedState>,
    p: &[Timed],
    repeated: RepeatedCommit,
) -> Result<u64, OutOfSnOrder> {
    if !p.iter().any(|t| !matches!(t.entry, Entry::Nop(_))) {
        return Ok(0);
    }
    if repeated == RepeatedCommit::Skip {
        state.check_sn_order(p.iter().map(|t| &t.entry))?;
    }
    let state = cow_state(state, CLONE_SITE_RECOVERY_COW_EXECUTE);
    Ok(p.iter()
        .filter(|t| !state.commit(&t.entry, repeated))
        .count() as u64)
}

/// Removes the first `len` entries (P) from L_i; an emptied log gets the dummy x_d. The kept
/// suffix keeps L_i's own stamps, and P's stamps never reach S_i.
pub(crate) fn strip_prefix(log: &mut Log, len: usize, round: u64) {
    log.strip_front(len, x_d(round));
}

/// P_i: the longest prefix of `log` whose entries are all at least `t` rounds old.
pub(crate) fn aged_prefix(log: &[Timed], round: u64, t: u64) -> Vec<Timed> {
    log[..aged_prefix_len(log, round, t)].to_vec()
}
