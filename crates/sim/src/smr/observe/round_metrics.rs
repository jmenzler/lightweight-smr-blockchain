//! Stage (f)'s per-round metrics row, shared by the compact and recovery round steps.

use crate::smr::SmrRoundMetrics;
use protocol::chunked::ChunkSeq;
use protocol::compact::Timed;

/// Stage (f)'s per-round row, read off the post-step logs and executed lengths.
pub(in crate::smr) fn smr_round_metrics(
    post_logs: &[Option<&ChunkSeq<Timed>>],
    executed_lens: &[usize],
    mask: &[bool],
    useful: u32,
    distinct_logs: u32,
    arrivals: u32,
) -> SmrRoundMetrics {
    let min_executed = executed_lens.iter().min().copied().unwrap_or(0) as u32;
    let useful_lens = || {
        (0..executed_lens.len())
            .filter(|&i| post_logs[i].is_some() && !mask[i])
            .map(|i| executed_lens[i] as u32)
    };
    SmrRoundMetrics {
        nonbot_logs: post_logs.iter().filter(|l| l.is_some()).count() as u32,
        blocked: mask.iter().filter(|&&b| b).count() as u32,
        useful,
        distinct_logs,
        max_log_len: post_logs
            .iter()
            .flatten()
            .map(|l| l.len())
            .max()
            .unwrap_or(0) as u32,
        min_executed_len: min_executed,
        max_executed_len: executed_lens.iter().max().copied().unwrap_or(0) as u32,
        useful_executed: useful_lens().min().zip(useful_lens().max()),
        arrivals,
        lcp_len: min_executed,
    }
}
