//! Real-socket rehearsal: n=5 node processes on loopback, both sync modes.
//! Slow + port-binding — run explicitly:
//! `cargo test -p node --test localhost -- --ignored`

use network_node::collect::aggregate;
use network_node::record::{PostState, RoundRecord};
use network_node::spec::NodeRunSpec;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const N: usize = 5;

/// Consecutive runs must not reuse ports (TIME_WAIT sockets from the previous
/// run refuse the bind and kill the node). Derive a per-process base.
fn port_base(slot: u16) -> u16 {
    20000 + ((std::process::id() as u16 % 400) * 20) + slot * 6
}
const ROUNDS: usize = 35;
/// Loopback on a contended dev machine is NOT the deployment tier — Shadow's
/// sim-time is. Δ here only has to exceed real scheduler jitter, or a loaded
/// machine trips the late-reply gate and looks like a protocol fault.
const ROUND_MS: u64 = 250;

fn write_run_files(dir: &std::path::Path, sync: &str, base_port: u16) -> (String, String) {
    let spec = format!(
        r#"{{"scenario":{{"median":{{"n":{N},"seed":11,"k":6,"ell":3,
             "init":{{"kind":"distinct"}},"max_rounds":{ROUNDS}}}}},
             "sync":"{sync}","round_ms":{ROUND_MS}}}"#
    );
    let spec_path = dir.join("spec.json");
    std::fs::File::create(&spec_path)
        .unwrap()
        .write_all(spec.as_bytes())
        .unwrap();
    let peers: String = (0..N)
        .map(|i| format!("127.0.0.1:{}\n", base_port + i as u16))
        .collect();
    let peers_path = dir.join("peers.txt");
    std::fs::File::create(&peers_path)
        .unwrap()
        .write_all(peers.as_bytes())
        .unwrap();
    (
        spec_path.to_string_lossy().into_owned(),
        peers_path.to_string_lossy().into_owned(),
    )
}

fn spawn_nodes(spec: &str, peers: &str, extra: &[(&str, String)]) -> Vec<Child> {
    (0..N)
        .map(|id| {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_node"));
            cmd.args([
                "--spec",
                spec,
                "--peers",
                peers,
                "--node-id",
                &id.to_string(),
            ]);
            for (k, v) in extra {
                cmd.args([*k, v.as_str()]);
            }
            cmd.stdout(Stdio::piped()).spawn().expect("spawn node")
        })
        .collect()
}

fn harvest(children: Vec<Child>) -> Vec<Vec<RoundRecord>> {
    children
        .into_iter()
        .map(|child| {
            let out = child.wait_with_output().expect("node exit");
            assert!(out.status.success(), "node exited nonzero");
            String::from_utf8(out.stdout)
                .expect("utf8")
                .lines()
                .map(|l| RoundRecord::from_jsonl(l).expect("jsonl line"))
                .collect()
        })
        .collect()
}

/// Gates that must be zero on ANY tier. Boundary-timing gates (late replies,
/// late appends) are only required to vanish under Shadow's sim-time — on a
/// contended dev machine they measure the machine, not the protocol.
fn assert_semantic_gates(gates: &network_node::collect::Gates) {
    assert!(gates.safety_ok, "executed-prefix consistency: {gates:?}");
    assert_eq!(gates.encode_err, 0, "encode faults: {gates:?}");
    assert_eq!(gates.transport_err, 0, "transport faults: {gates:?}");
    assert_eq!(gates.dropped_past, 0, "past-round messages: {gates:?}");
    assert!(gates.rounds_consistent, "ragged round counts: {gates:?}");
    assert_eq!(gates.client_lost, 0, "client transport faults: {gates:?}");
    assert_eq!(
        gates.client_starved, 0,
        "starved client attempts: {gates:?}"
    );
    assert_eq!(gates.early_cli, 0, "early client commands: {gates:?}");
    assert_eq!(
        gates.cli_unreconciled, 0,
        "client sends vs server counts: {gates:?}"
    );
}

fn assert_clean_convergence(records: Vec<Vec<RoundRecord>>, spec_json: &str) {
    let spec = NodeRunSpec::from_json(spec_json).unwrap();
    let out = aggregate(&records, &spec).expect("aggregate");
    assert_semantic_gates(&out.gates);
    assert!(
        out.summary.convergence_round.is_some(),
        "n=5 distinct under zero blocking converges"
    );
}

#[test]
#[ignore]
fn timer_mode_converges_with_zero_gates() {
    let dir = std::env::temp_dir().join(format!("node-timer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (spec, peers) = write_run_files(&dir, "timer", port_base(0));
    let start_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 2500;
    let children = spawn_nodes(&spec, &peers, &[("--start-at-ms", start_at.to_string())]);
    let records = harvest(children);
    assert_clean_convergence(records, &std::fs::read_to_string(&spec).unwrap());
}

#[test]
#[ignore]
fn barrier_mode_converges_with_zero_gates() {
    let dir = std::env::temp_dir().join(format!("node-barrier-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (spec, peers) = write_run_files(&dir, "barrier", port_base(1));

    let coordinator = Command::new(env!("CARGO_BIN_EXE_coordinator"))
        .args([
            "--port",
            &port_base(1).saturating_add(5).to_string(),
            "--n",
            &N.to_string(),
            "--rounds",
            &ROUNDS.to_string(),
        ])
        .spawn()
        .expect("spawn coordinator");
    std::thread::sleep(Duration::from_millis(300));

    let children = spawn_nodes(
        &spec,
        &peers,
        &[(
            "--coordinator",
            format!("127.0.0.1:{}", port_base(1).saturating_add(5)),
        )],
    );
    let records = harvest(children);
    let status = coordinator.wait_with_output().expect("coordinator exit");
    assert!(status.status.success(), "coordinator exited nonzero");
    assert_clean_convergence(records, &std::fs::read_to_string(&spec).unwrap());
}

#[test]
#[ignore]
fn compact_smr_with_client_driver_commits_the_injection() {
    let dir = std::env::temp_dir().join(format!("node-smr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = format!(
        r#"{{"scenario":{{"smr":{{"n":{N},"seed":5,"k":6,"ell":3,"sigma":1.0,
             "proto":{{"kind":"compact","t_commit_rounds":4}},
             "injections":[{{"round":2,"client":1,"op":700}}],
             "max_rounds":{ROUNDS}}}}},
             "sync":"timer","round_ms":{ROUND_MS},"tracked_ops":[700]}}"#
    );
    let spec_path = dir.join("spec.json");
    std::fs::write(&spec_path, &spec).unwrap();
    let peers: String = (0..N)
        .map(|i| format!("127.0.0.1:{}\n", 19170 + i as u16))
        .collect();
    let peers_path = dir.join("peers.txt");
    std::fs::write(&peers_path, &peers).unwrap();
    let (spec_s, peers_s) = (
        spec_path.to_string_lossy().into_owned(),
        peers_path.to_string_lossy().into_owned(),
    );

    let start_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 2500;
    let extra = [("--start-at-ms", start_at.to_string())];
    let nodes = spawn_nodes(&spec_s, &peers_s, &extra);
    let mut client = Command::new(env!("CARGO_BIN_EXE_client_driver"));
    client.args([
        "--spec",
        &spec_s,
        "--peers",
        &peers_s,
        "--start-at-ms",
        &start_at.to_string(),
    ]);
    let client = client.stdout(Stdio::piped()).spawn().expect("spawn client");

    let records = harvest(nodes);
    let client_out = client.wait_with_output().expect("client exit");
    assert!(client_out.status.success());
    let client_stdout = String::from_utf8(client_out.stdout).unwrap();

    let nspec = NodeRunSpec::from_json(&spec).unwrap();
    let client = network_node::collect::ClientLog::parse(&client_stdout).expect("client log");
    let out =
        network_node::collect::aggregate_smr(&records, &nspec, Some(&client)).expect("aggregate");
    assert_semantic_gates(&out.gates);
    assert!(
        client_stdout.contains("\"terminal\":true"),
        "command must reach AckCommitted: {client_stdout}"
    );
    assert!(
        !client_stdout.contains("\"committed_ack_round\":null"),
        "committed_ack_round must carry a real round: {client_stdout}"
    );
    assert!(
        !client_stdout.contains("\"starved\":1"),
        "no client attempt may miss its round: {client_stdout}"
    );
    // the tracked op must be in the executed sequence (position < exec_len), not merely in the
    // uncommitted log, on every node by the horizon
    for per_node in &records {
        let last = per_node.last().unwrap();
        let PostState::Log { exec_len, .. } = last.post else {
            panic!("log mode expected, got {:?}", last.post);
        };
        assert!(
            last.tracked[0].pos.is_some_and(|p| p < exec_len),
            "op 700 executed everywhere at horizon: pos {:?}, exec_len {exec_len}",
            last.tracked[0].pos
        );
    }
}

/// A pinned injection (`target`) must address only that server, on every
/// attempt — client_driver honoring `Injection.target` the way the sim
/// engine does (crates/sim/src/smr.rs stage (b)).
#[test]
#[ignore]
fn pinned_injection_reaches_only_its_target() {
    const NPIN: usize = 3;
    let dir = std::env::temp_dir().join(format!("node-smr-pin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = format!(
        r#"{{"scenario":{{"smr":{{"n":{NPIN},"seed":5,"k":6,"ell":3,"sigma":1.0,
             "proto":{{"kind":"compact","t_commit_rounds":4}},
             "injections":[{{"round":2,"client":1,"op":700,"target":2}}],
             "max_rounds":{ROUNDS}}}}},
             "sync":"timer","round_ms":{ROUND_MS},"tracked_ops":[700]}}"#
    );
    let spec_path = dir.join("spec.json");
    std::fs::write(&spec_path, &spec).unwrap();
    let peers: String = (0..NPIN)
        .map(|i| format!("127.0.0.1:{}\n", 19270 + i as u16))
        .collect();
    let peers_path = dir.join("peers.txt");
    std::fs::write(&peers_path, &peers).unwrap();
    let (spec_s, peers_s) = (
        spec_path.to_string_lossy().into_owned(),
        peers_path.to_string_lossy().into_owned(),
    );

    let start_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 2500;
    let nodes: Vec<Child> = (0..NPIN)
        .map(|id| {
            Command::new(env!("CARGO_BIN_EXE_node"))
                .args([
                    "--spec",
                    &spec_s,
                    "--peers",
                    &peers_s,
                    "--node-id",
                    &id.to_string(),
                    "--start-at-ms",
                    &start_at.to_string(),
                ])
                .stdout(Stdio::piped())
                .spawn()
                .expect("spawn node")
        })
        .collect();
    let mut client = Command::new(env!("CARGO_BIN_EXE_client_driver"));
    client.args([
        "--spec",
        &spec_s,
        "--peers",
        &peers_s,
        "--start-at-ms",
        &start_at.to_string(),
    ]);
    let client = client.stdout(Stdio::piped()).spawn().expect("spawn client");

    let records = harvest(nodes);
    let client_out = client.wait_with_output().expect("client exit");
    assert!(client_out.status.success());
    let client_stdout = String::from_utf8(client_out.stdout).unwrap();
    assert!(
        client_stdout.contains("\"terminal\":true"),
        "the pinned command must still reach AckCommitted: {client_stdout}"
    );

    let nspec = NodeRunSpec::from_json(&spec).unwrap();
    let client = network_node::collect::ClientLog::parse(&client_stdout).expect("client log");
    let out =
        network_node::collect::aggregate_smr(&records, &nspec, Some(&client)).expect("aggregate");
    assert_semantic_gates(&out.gates);

    let cli_recv_total: Vec<u32> = records
        .iter()
        .map(|per_node| per_node.iter().map(|r| r.cli_recv).sum())
        .collect();
    assert!(
        cli_recv_total[2] > 0,
        "the pinned server (id 2) must receive the client command: {cli_recv_total:?}"
    );
    for (id, &total) in cli_recv_total.iter().enumerate() {
        if id != 2 {
            assert_eq!(
                total, 0,
                "server {id} is not the pinned target and must see no ClientCmd: {cli_recv_total:?}"
            );
        }
    }
}
