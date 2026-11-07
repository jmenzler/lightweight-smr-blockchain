//! Per-round JSONL record, one line per node per round on stdout.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PostState {
    Value(Option<u64>),
    Log {
        log_len: Option<usize>,
        exec_len: usize,
        log_hash: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackedDigest {
    pub op: u64,
    /// Executed or in the log: the position and prefix digests below are defined.
    pub present: bool,
    /// Still in the log; a committed entry has left it.
    pub in_log: bool,
    pub pos: Option<usize>,
    pub prefix_hash: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundRecord {
    pub r: u64,
    pub blocked: bool,
    /// Held a servable (non-⊥) snapshot entering the round.
    pub snap_held: bool,
    pub post: PostState,
    pub replies: u16,
    pub req_sent: u32,
    /// Pull requests that reached this node for this round, blocked or not.
    pub req_recv: u32,
    pub rep_sent: u32,
    pub rep_recv: u32,
    pub app_sent: u32,
    pub app_recv: u32,
    /// Appends that reached this node for this round while it was blocked (loss the model allows).
    pub app_blocked: u32,
    pub cli_recv: u32,
    pub acks: u32,
    pub bytes_out: u64,
    pub bytes_in: u64,
    pub late_rep: u32,
    pub late_app: u32,
    pub dropped_past: u32,
    /// Client commands tagged for a round that had already ended.
    pub late_cli: u32,
    /// Client commands tagged two or more rounds ahead, which the transport cannot hold.
    pub early_cli: u32,
    /// Client commands that reached this node while it was blocked (loss the model allows).
    pub cli_blocked: u32,
    /// Alg 5 under the skip policy refused a commit that skips a client sn; the node is frozen from this
    /// round on (the core is never stepped again) and the run's safety check fails.
    pub sn_order_violation: bool,
    /// Encode failures (oversized frame): a config fault, never protocol silence.
    pub encode_err: u32,
    /// Connect/read/write/decode failures seen by the transport shell: loss the model does not have.
    pub transport_err: u32,
    /// (executed length, chained prefix hash) per entry executed this round; compact only.
    #[serde(default)]
    pub exec_prefix: Vec<(usize, u64)>,
    pub t_last_reply_us: u64,
    pub t_step_done_us: u64,
    /// Time spent in `RoundEngine::on_round_end()` alone.
    pub t_step_compute_us: u64,
    pub tracked: Vec<TrackedDigest>,
}

impl RoundRecord {
    pub fn to_jsonl(&self) -> String {
        serde_json::to_string(self).expect("record serializes")
    }

    pub fn from_jsonl(line: &str) -> Result<RoundRecord, String> {
        serde_json::from_str(line).map_err(|e| e.to_string())
    }
}
