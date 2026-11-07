//! One client attempt: send a command to one server and classify what came back.

use crate::net::read_frame;
use crate::wire::{Msg, decode};
use std::io::ErrorKind;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

#[derive(Debug)]
pub enum Attempt {
    Reply(Msg),
    /// Connected and sent, then the server closed cleanly without an ack: blocked or ignoring, as in the model.
    Silent,
    /// Connect, write, read or decode failed: a message the model does not lose. `written` tells whether
    /// the command left the client, so it can still be reconciled against the server's counters.
    Lost {
        written: bool,
    },
    Starved,
}

impl Attempt {
    /// Whether the command reached the wire; `Starved` is cut off at an unknown point.
    pub fn written(&self) -> bool {
        matches!(
            self,
            Attempt::Reply(_) | Attempt::Silent | Attempt::Lost { written: true }
        )
    }
}

pub async fn attempt(addr: &str, frame: &[u8], max_frame: u32) -> Attempt {
    let Ok(mut stream) = TcpStream::connect(addr).await else {
        return Attempt::Lost { written: false };
    };
    if stream.write_all(frame).await.is_err() {
        return Attempt::Lost { written: false };
    }
    let reply = match read_frame(&mut stream, max_frame).await {
        Ok(reply) => reply,
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Attempt::Silent,
        Err(e) => {
            eprintln!("client: ack read FAILED: {e}");
            return Attempt::Lost { written: true };
        }
    };
    match decode(&reply, max_frame) {
        Ok(msg) => Attempt::Reply(msg),
        Err(e) => {
            eprintln!("client: ack decode FAILED: {e}");
            Attempt::Lost { written: true }
        }
    }
}
