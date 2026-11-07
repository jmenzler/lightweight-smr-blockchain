//! Barrier coordinator: per round, collect one RoundDone from every node, then broadcast RoundGo.
//!
//! Every connection opens with a hello `RoundDone { round: 0, from }`; `from >= n` marks an observer
//! (the client driver) that only receives the RoundGo stream and so follows the nodes' rounds.

use network_node::cli::arg;
use network_node::net::read_frame;
use network_node::wire::{Msg, decode, encode};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

const MAX_FRAME: u32 = 1024 * 1024;
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 60_000;

fn die(msg: &str) -> ! {
    network_node::cli::die("coordinator", msg)
}

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(run());
}

async fn run() {
    let port: u16 = arg("--port")
        .unwrap_or_else(|| die("--port required"))
        .parse()
        .unwrap_or_else(|_| die("--port"));
    let n: usize = arg("--n")
        .unwrap_or_else(|| die("--n required"))
        .parse()
        .unwrap_or_else(|_| die("--n"));
    let rounds: u64 = arg("--rounds")
        .unwrap_or_else(|| die("--rounds required"))
        .parse()
        .unwrap_or_else(|_| die("--rounds"));

    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap_or_else(|e| die(&format!("bind :{port}: {e}")));
    let observers: usize = arg("--observers").map_or(0, |v| {
        v.parse()
            .unwrap_or_else(|_| die("--observers must be an integer"))
    });
    let connect_timeout = Duration::from_millis(arg("--connect-timeout-ms").map_or(
        DEFAULT_CONNECT_TIMEOUT_MS,
        |v| {
            v.parse()
                .unwrap_or_else(|_| die("--connect-timeout-ms must be an integer"))
        },
    ));
    let mut conns: Vec<TcpStream> = Vec::with_capacity(n);
    let mut watchers: Vec<TcpStream> = Vec::with_capacity(observers);
    let mut seen = vec![false; n];
    let gather = async {
        while conns.len() < n || watchers.len() < observers {
            let mut stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(e) => die(&format!("accept: {e}")),
            };
            let hello = read_frame(&mut stream, MAX_FRAME)
                .await
                .map(|f| decode(&f, MAX_FRAME));
            match hello {
                Ok(Ok(Msg::RoundDone { round: 0, from })) if (from as usize) < n => {
                    if std::mem::replace(&mut seen[from as usize], true) {
                        die(&format!("node {from} connected twice"));
                    }
                    conns.push(stream);
                }
                Ok(Ok(Msg::RoundDone { round: 0, .. })) => watchers.push(stream),
                other => die(&format!("bad hello: {other:?}")),
            }
        }
    };
    if tokio::time::timeout(connect_timeout, gather).await.is_err() {
        die(&format!(
            "timed out after {} ms waiting for participants: {} of {n} nodes and {} of {observers} \
             observers connected",
            connect_timeout.as_millis(),
            conns.len(),
            watchers.len()
        ));
    }

    // every node is connected, hence its listener is bound: round 1 may start
    let start = encode(&Msg::RoundGo { round: 0 }, MAX_FRAME, 0).expect("control frame");
    for stream in conns.iter_mut() {
        if stream.write_all(&start).await.is_err() {
            die("start RoundGo write failed");
        }
    }
    for stream in watchers.iter_mut() {
        if stream.write_all(&start).await.is_err() {
            die("start RoundGo write to an observer failed");
        }
    }

    for round in 1..=rounds {
        // accept order is unrelated to node ids, so completion is tracked by connection
        for (i, stream) in conns.iter_mut().enumerate() {
            let frame = read_frame(stream, MAX_FRAME)
                .await
                .unwrap_or_else(|e| die(&format!("round {round}: node conn {i}: {e}")));
            match decode(&frame, MAX_FRAME) {
                Ok(Msg::RoundDone { round: r, .. }) if r == round => {}
                other => die(&format!("round {round}: unexpected {other:?}")),
            }
        }
        let go = encode(&Msg::RoundGo { round }, MAX_FRAME, 0).expect("control frame");
        for stream in conns.iter_mut() {
            if stream.write_all(&go).await.is_err() {
                die(&format!("round {round}: RoundGo write failed"));
            }
        }
        // an observer that already finished may have hung up
        for stream in watchers.iter_mut() {
            let _ = stream.write_all(&go).await;
        }
    }
}
