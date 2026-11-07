//! How one client attempt is classified: only a clean close before any reply is the model's silence.

use network_node::client::{Attempt, attempt};
use network_node::net::{read_frame, read_message};
use network_node::wire::{AckKind, Msg, encode};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

const MAX_FRAME: u32 = 1 << 20;

fn frame() -> Vec<u8> {
    let cmd = Msg::ClientCmd {
        round: 1,
        client: 1,
        sn: 1,
        op: 9,
    };
    encode(&cmd, MAX_FRAME, 0).expect("frame")
}

/// Serves exactly one connection with `serve`, after reading the command frame.
async fn attempt_against<F, Fut>(serve: F) -> Attempt
where
    F: FnOnce(tokio::net::TcpStream) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        read_frame(&mut stream, MAX_FRAME).await.expect("command");
        serve(stream).await;
    });
    attempt(&addr, &frame(), MAX_FRAME).await
}

#[tokio::test]
async fn a_clean_close_without_a_reply_is_silent() {
    let got = attempt_against(|stream| async move { drop(stream) }).await;
    assert!(matches!(got, Attempt::Silent), "{got:?}");
}

#[tokio::test]
#[allow(deprecated)]
async fn a_connection_reset_after_the_write_is_lost() {
    let got = attempt_against(|stream| async move {
        stream
            .set_linger(Some(std::time::Duration::ZERO))
            .expect("linger");
        drop(stream);
    })
    .await;
    assert!(matches!(got, Attempt::Lost { written: true }), "{got:?}");
}

#[tokio::test]
async fn a_frame_cut_off_mid_body_is_lost() {
    let got = attempt_against(|mut stream| async move {
        stream
            .write_all(&[200, 0, 0, 0, 1, 2])
            .await
            .expect("write");
    })
    .await;
    assert!(matches!(got, Attempt::Lost { written: true }), "{got:?}");
}

#[tokio::test]
async fn an_undecodable_ack_is_lost() {
    let got = attempt_against(|mut stream| async move {
        stream
            .write_all(&[2, 0, 0, 0, 0xff, 0xff])
            .await
            .expect("write");
    })
    .await;
    assert!(matches!(got, Attempt::Lost { written: true }), "{got:?}");
}

#[tokio::test]
async fn an_unreachable_server_is_lost_before_the_write() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    drop(listener);
    let got = attempt(&addr, &frame(), MAX_FRAME).await;
    assert!(matches!(got, Attempt::Lost { written: false }), "{got:?}");
}

#[tokio::test]
async fn an_ack_is_a_reply() {
    let ack = Msg::ClientAck {
        round: 1,
        client: 1,
        sn: 1,
        kind: AckKind::Delivered,
    };
    let got = attempt_against(move |mut stream| async move {
        stream
            .write_all(&encode(&ack, MAX_FRAME, 0).expect("ack"))
            .await
            .expect("write");
    })
    .await;
    assert!(
        matches!(got, Attempt::Reply(Msg::ClientAck { .. })),
        "{got:?}"
    );
}

#[tokio::test]
async fn a_padded_command_and_its_ack_survive_an_unread_padding_tail() {
    let padded = encode(
        &Msg::ClientCmd {
            round: 1,
            client: 1,
            sn: 1,
            op: 9,
        },
        MAX_FRAME,
        4096,
    )
    .expect("frame");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        read_frame(&mut stream, MAX_FRAME).await.expect("command");
        let ack = Msg::ClientAck {
            round: 1,
            client: 1,
            sn: 1,
            kind: AckKind::Delivered,
        };
        stream
            .write_all(&encode(&ack, MAX_FRAME, 0).expect("ack"))
            .await
            .expect("write");
    });
    let got = attempt(&addr, &padded, MAX_FRAME).await;
    assert!(matches!(got, Attempt::Reply(_)), "{got:?}");
}

#[tokio::test]
async fn a_padded_command_at_a_silent_server_is_silent_not_lost() {
    let padded = encode(
        &Msg::ClientCmd {
            round: 1,
            client: 1,
            sn: 1,
            op: 9,
        },
        MAX_FRAME,
        4096,
    )
    .expect("frame");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        read_message(&mut stream, MAX_FRAME, 4096)
            .await
            .expect("command");
    });
    let got = attempt(&addr, &padded, MAX_FRAME).await;
    assert!(matches!(got, Attempt::Silent), "{got:?}");
}
