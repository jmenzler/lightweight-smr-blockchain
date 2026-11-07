//! All SMR clients in one process: each pending command is resent to one random server per round until its terminal ack.
//!
//! An Alg 3 command stops after its first `Delivered` ack, i.e. one injection at the first non-blocked
//! server reached, even a ⊥ one; if that server's amplification is then lost, the command is not
//! retried. The paper leaves the resend count and stop rule open (A3.OPEN.25, pp. 19 and 25), and the
//! simulator's extended client stops the same way. An Alg 5 command resends until `AckCommitted`.
//!
//! Timer mode runs on the shared `--start-at-ms` grid. Barrier mode follows the nodes' rounds through the
//! coordinator's RoundGo stream (`--coordinator`), so client and node round numbers cannot drift apart.

use network_node::cli::arg;
use network_node::client::{Attempt, attempt};
use network_node::net::read_frame;
use network_node::rng::derive_rng;
use network_node::spec::{NodeRunSpec, ScenarioKind, SyncMode};
use network_node::wire::{AckKind, Msg, decode, encode};
use rand::Rng;
use sim::smr::{active_phase, auto_command, injection_sns};
use sim::smr_commands::{CommandRow, commands_line};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

fn die(msg: &str) -> ! {
    network_node::cli::die("client_driver", msg)
}

struct Pending {
    client: u32,
    sn: u64,
    op: u64,
    injection_round: u64,
    target: Option<u32>,
    delivered_round: Option<u64>,
    committed_ack_round: Option<u64>,
    terminal: bool,
}

async fn await_round_go(stream: &mut TcpStream, round: u64, max_frame: u32) {
    match read_frame(stream, max_frame)
        .await
        .map(|f| decode(&f, max_frame))
    {
        Ok(Ok(Msg::RoundGo { round: go })) if go == round => {}
        other => die(&format!("barrier desync at round {round}: {other:?}")),
    }
}

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(run());
}

async fn run() {
    let spec_path = arg("--spec").unwrap_or_else(|| die("--spec required"));
    let peers_path = arg("--peers").unwrap_or_else(|| die("--peers required"));
    let spec_json =
        std::fs::read_to_string(&spec_path).unwrap_or_else(|e| die(&format!("{spec_path}: {e}")));
    let spec = NodeRunSpec::from_json(&spec_json).unwrap_or_else(|e| die(&format!("spec: {e}")));
    let ScenarioKind::Smr(smr) = &spec.scenario else {
        die("client_driver drives SMR scenarios only");
    };
    let scenario =
        sim::smr::SmrScenario::try_from(smr.clone()).unwrap_or_else(|e| die(&e.to_string()));
    let compact = matches!(scenario.proto, sim::smr::Proto::Compact { .. });
    let peers: Vec<String> = std::fs::read_to_string(&peers_path)
        .unwrap_or_else(|e| die(&format!("{peers_path}: {e}")))
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let n = peers.len();
    if n != scenario.n {
        die(&format!("peers {} != n {}", n, scenario.n));
    }

    let sns = injection_sns(&scenario.injections);
    let mut pending: Vec<Pending> = scenario
        .injections
        .iter()
        .zip(&sns)
        .map(|(inj, &sn)| Pending {
            client: inj.client,
            sn,
            op: inj.op,
            injection_round: inj.round as u64,
            target: inj.target,
            delivered_round: None,
            committed_ack_round: None,
            terminal: false,
        })
        .collect();

    let mut traffic_rng = derive_rng(scenario.seed, "traffic", 0);
    let mut target_rng = derive_rng(scenario.seed, "client", 0);
    let max_frame = spec.max_frame_bytes;
    let round_ms = spec.round_ms;
    let max_rounds = scenario.max_rounds as u64;

    let mut coord = match spec.sync {
        SyncMode::Barrier => {
            let addr = arg("--coordinator")
                .unwrap_or_else(|| die("--coordinator required in barrier mode"));
            let mut stream = TcpStream::connect(addr.as_str())
                .await
                .unwrap_or_else(|e| die(&format!("coordinator {addr}: {e}")));
            let hello = encode(
                &Msg::RoundDone {
                    round: 0,
                    from: u32::MAX,
                },
                max_frame,
                0,
            )
            .expect("control frame");
            stream
                .write_all(&hello)
                .await
                .unwrap_or_else(|e| die(&format!("coordinator hello: {e}")));
            await_round_go(&mut stream, 0, max_frame).await;
            Some(stream)
        }
        SyncMode::Timer => None,
    };

    let t0 = match arg("--start-at-ms") {
        Some(ms) => {
            let target = ms.parse::<u64>().unwrap_or_else(|_| die("--start-at-ms"));
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_millis() as u64;
            tokio::time::Instant::now() + Duration::from_millis(target.saturating_sub(now))
        }
        None => tokio::time::Instant::now() + Duration::from_millis(spec.start_offset_ms),
    };

    if coord.is_none() {
        tokio::time::sleep_until(t0).await;
    }

    let mut auto_idx: u64 = 0;
    for round in 1..=max_rounds {
        // (a0) one pmf draw per round while a phase is active.
        let mut arrivals = 0usize;
        if let Some(phase) = scenario
            .traffic
            .as_deref()
            .and_then(|phases| active_phase(phases, round as usize))
        {
            arrivals = sim::draw_weighted_index(&mut traffic_rng, &phase.arrivals_pmf);
            for _ in 0..arrivals {
                let cc = auto_command(auto_idx);
                pending.push(Pending {
                    client: cc.client,
                    sn: cc.sn,
                    op: cc.op,
                    injection_round: round,
                    target: None,
                    delivered_round: None,
                    committed_ack_round: None,
                    terminal: false,
                });
                auto_idx += 1;
            }
        }
        println!("{{\"r\":{round},\"arrivals\":{arrivals}}}");

        // sim stage (b): all targets are drawn first, in pending order, so the stream ignores network timing
        let deadline = match coord {
            Some(_) => tokio::time::Instant::now() + Duration::from_millis(round_ms),
            None => t0 + Duration::from_millis(round_ms * round),
        };
        let due: Vec<usize> = (0..pending.len())
            .filter(|&i| !pending[i].terminal && pending[i].injection_round <= round)
            .collect();
        let attempts: Vec<(usize, Vec<u8>, String)> = due
            .iter()
            .map(|&i| {
                let cmd = &pending[i];
                let target = match cmd.target {
                    Some(t) => t as usize,
                    None => target_rng.random_range(0..n),
                };
                let addr = peers[target].clone();
                let msg = Msg::ClientCmd {
                    round,
                    client: cmd.client,
                    sn: cmd.sn,
                    op: cmd.op,
                };
                let frame = encode(&msg, max_frame, spec.payload_bytes)
                    .unwrap_or_else(|e| die(&format!("ClientCmd encode: {e}")));
                (i, frame, addr)
            })
            .collect();

        let mut tasks = Vec::with_capacity(attempts.len());
        for (i, frame, addr) in attempts {
            tasks.push(tokio::spawn(async move {
                let outcome = tokio::time::timeout_at(deadline, attempt(&addr, &frame, max_frame))
                    .await
                    .unwrap_or(Attempt::Starved);
                (i, outcome)
            }));
        }
        let mut starved = 0u32;
        let mut lost = 0u32;
        let mut sent = 0u32;
        for task in tasks {
            let Ok((i, outcome)) = task.await else {
                die("client attempt task panicked");
            };
            sent += u32::from(outcome.written());
            match outcome {
                Attempt::Starved => starved += 1,
                Attempt::Lost { .. } => lost += 1,
                Attempt::Silent => {}
                Attempt::Reply(Msg::ClientAck { kind, .. }) => {
                    let cmd = &mut pending[i];
                    match kind {
                        AckKind::Delivered => {
                            cmd.delivered_round.get_or_insert(round);
                            if !compact {
                                cmd.terminal = true;
                            }
                        }
                        AckKind::Amplified => {
                            cmd.delivered_round.get_or_insert(round);
                        }
                        AckKind::AckCommitted => {
                            cmd.committed_ack_round.get_or_insert(round);
                            cmd.terminal = true;
                        }
                        AckKind::Ignored => {}
                    }
                }
                Attempt::Reply(_) => {}
            }
        }
        if starved > 0 {
            eprintln!("client_driver: round {round}: {starved} attempts hit the deadline");
        }
        if lost > 0 {
            eprintln!("client_driver: round {round}: {lost} attempts failed in transport");
        }
        println!("{{\"r\":{round},\"sent\":{sent}}}");
        println!("{{\"r\":{round},\"lost\":{lost}}}");
        println!("{{\"r\":{round},\"starved\":{starved}}}");
        match coord.as_mut() {
            Some(stream) => await_round_go(stream, round, max_frame).await,
            None => tokio::time::sleep_until(deadline).await,
        }
    }

    let rows: Vec<CommandRow> = pending
        .iter()
        .map(|c| CommandRow {
            client: c.client,
            sn: c.sn,
            op: c.op,
            injection_round: c.injection_round,
            delivered_round: c.delivered_round,
            committed_ack_round: c.committed_ack_round,
            terminal: c.terminal,
        })
        .collect();
    println!("{}", commands_line(&rows));
}
