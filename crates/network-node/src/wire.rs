//! Wire protocol: `[u32 LE body_len][bincode-1 fixint body][padding]` frames; padding is ignored on decode.

use crate::NodeId;
use protocol::compact::{SharedState, Timed};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReplyPayload {
    Value(u64),
    Log(Vec<u64>),
    Compact {
        log: Vec<Timed>,
        state: Option<SharedState>,
    },
}

/// Client acknowledgements on the request's own connection. Only the commit ack exists in the paper
/// (Alg 5, "until commitment is acknowledged", p. 25); the rest is instrumentation that never changes
/// server state. `Delivered`: an Alg 3 server accepted the command, which ends the client (Alg 3 has
/// no commit, resend policy open, A3.OPEN.25). `Amplified`: an Alg 5 server holding a log accepted
/// it, which sets the client's delivery round. `Ignored`: an Alg 5 server at ⊥ discards the command
/// as the paper says, but answers explicitly where the paper's server stays silent; it does not
/// count as injection (M.OPEN.13) and does not stop the resends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AckKind {
    Delivered,
    Amplified,
    AckCommitted,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    PullRequest {
        round: u64,
        from: NodeId,
        slot: u16,
        needs_state: bool,
    },
    PullReply {
        round: u64,
        from: NodeId,
        slot: u16,
        payload: ReplyPayload,
    },
    Append {
        round: u64,
        from: NodeId,
        client: u32,
        sn: u64,
        op: u64,
    },
    ClientCmd {
        round: u64,
        client: u32,
        sn: u64,
        op: u64,
    },
    ClientAck {
        round: u64,
        client: u32,
        sn: u64,
        kind: AckKind,
    },
    RoundDone {
        round: u64,
        from: NodeId,
    },
    RoundGo {
        round: u64,
    },
}

/// Bytes of zero padding that trail the frame body: one `payload_pad` per carried entry.
pub fn padding_len(msg: &Msg, payload_pad: u32) -> usize {
    entry_count(msg) * payload_pad as usize
}

fn entry_count(msg: &Msg) -> usize {
    match msg {
        Msg::PullReply { payload, .. } => match payload {
            ReplyPayload::Value(_) => 1,
            ReplyPayload::Log(v) => v.len(),
            ReplyPayload::Compact { log, .. } => log.len(),
        },
        Msg::Append { .. } | Msg::ClientCmd { .. } => 1,
        Msg::PullRequest { .. }
        | Msg::ClientAck { .. }
        | Msg::RoundDone { .. }
        | Msg::RoundGo { .. } => 0,
    }
}

pub fn encode(msg: &Msg, max_frame_bytes: u32, payload_pad: u32) -> Result<Vec<u8>, String> {
    let body = bincode::serialize(msg).map_err(|e| e.to_string())?;
    let pad = padding_len(msg, payload_pad);
    let total = 4 + body.len() + pad;
    if total > max_frame_bytes as usize {
        return Err(format!(
            "frame of {total} bytes exceeds the {max_frame_bytes}-byte cap"
        ));
    }
    let mut frame = Vec::with_capacity(total);
    frame.extend_from_slice(&(u32::try_from(body.len()).map_err(|e| e.to_string())?).to_le_bytes());
    frame.extend_from_slice(&body);
    frame.resize(total, 0);
    Ok(frame)
}

/// Decode one full frame back into a message; trailing padding is ignored.
pub fn decode(frame: &[u8], max_frame_bytes: u32) -> Result<Msg, String> {
    if frame.len() < 4 {
        return Err("frame shorter than the length prefix".to_string());
    }
    let body_len = u32::from_le_bytes(frame[..4].try_into().expect("4 bytes")) as usize;
    if body_len + 4 > max_frame_bytes as usize {
        return Err(format!(
            "declared body of {body_len} bytes exceeds the {max_frame_bytes}-byte cap"
        ));
    }
    let body = frame.get(4..4 + body_len).ok_or_else(|| {
        format!(
            "frame truncated: {} of {body_len} body bytes",
            frame.len() - 4
        )
    })?;
    bincode::deserialize(body).map_err(|e| e.to_string())
}
