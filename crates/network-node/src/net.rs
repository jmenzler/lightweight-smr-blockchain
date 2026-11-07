//! Async frame readers.

use crate::wire::{Msg, decode, padding_len};
use std::io::{Error, ErrorKind, Result};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// A peer that closes before the first prefix byte surfaces as `UnexpectedEof`; any later cut-off
/// is `InvalidData`, so callers can tell a clean close from a torn frame.
pub async fn read_frame(stream: &mut TcpStream, max_frame_bytes: u32) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let body_len = u32::from_le_bytes(len_buf) as usize;
    if body_len + 4 > max_frame_bytes as usize {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("declared body of {body_len} bytes exceeds the {max_frame_bytes}-byte cap"),
        ));
    }
    let mut frame = vec![0u8; 4 + body_len];
    frame[..4].copy_from_slice(&len_buf);
    stream
        .read_exact(&mut frame[4..])
        .await
        .map_err(|e| match e.kind() {
            ErrorKind::UnexpectedEof => Error::new(
                ErrorKind::InvalidData,
                format!("frame truncated: peer closed inside a {body_len}-byte body"),
            ),
            _ => e,
        })?;
    Ok(frame)
}

/// Reads one frame, decodes it and consumes its trailing padding. Closing a socket with unread
/// bytes sends a reset, which a peer waiting for a reply would mistake for loss instead of silence.
pub async fn read_message(
    stream: &mut TcpStream,
    max_frame_bytes: u32,
    payload_pad: u32,
) -> Result<Msg> {
    let frame = read_frame(stream, max_frame_bytes).await?;
    let msg = decode(&frame, max_frame_bytes).map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
    let mut padding = vec![0u8; padding_len(&msg, payload_pad)];
    stream.read_exact(&mut padding).await?;
    Ok(msg)
}
