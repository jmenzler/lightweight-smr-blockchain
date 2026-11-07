//! Real-socket barrier run: the client driver follows the nodes' rounds.

use network_node::record::RoundRecord;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn free_base(slot: u16) -> u16 {
    24000 + ((std::process::id() as u16 % 300) * 30) + slot * 10
}

fn parse_records(out: Vec<u8>) -> Vec<RoundRecord> {
    String::from_utf8(out)
        .expect("utf8")
        .lines()
        .filter_map(|l| RoundRecord::from_jsonl(l).ok())
        .collect()
}

fn spawn_node(spec: &str, peers: &str, id: usize, extra: &[(&str, String)]) -> Child {
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
}

// Injection and delivery rounds use the nodes' round numbering (p. 19), not a drifting client clock.
#[test]
#[ignore = "slow: cargo test --release -- --ignored"]
fn barrier_client_delivered_round_equals_the_receiving_nodes_round() {
    const N: usize = 3;
    const ROUNDS: usize = 70;
    const INJECT: usize = 55;
    let dir = std::env::temp_dir().join(format!("localhost-barrier-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = format!(
        r#"{{"scenario":{{"smr":{{"n":{N},"seed":5,"k":6,"ell":3,"sigma":1.0,
             "proto":{{"kind":"compact","t_commit_rounds":4}},
             "injections":[{{"round":{INJECT},"client":1,"op":700,"target":2}}],
             "max_rounds":{ROUNDS}}}}},
             "sync":"barrier","round_ms":40,"tracked_ops":[700]}}"#
    );
    let spec_path = dir.join("spec.json");
    std::fs::write(&spec_path, &spec).unwrap();
    let base = free_base(1);
    let peers: String = (0..N)
        .map(|i| format!("127.0.0.1:{}\n", base + i as u16))
        .collect();
    let peers_path = dir.join("peers.txt");
    std::fs::write(&peers_path, peers).unwrap();
    let (spec_s, peers_s) = (
        spec_path.to_string_lossy().into_owned(),
        peers_path.to_string_lossy().into_owned(),
    );
    let coord_port = base + 8;
    let coordinator = Command::new(env!("CARGO_BIN_EXE_coordinator"))
        .args([
            "--port",
            &coord_port.to_string(),
            "--n",
            &N.to_string(),
            "--rounds",
            &ROUNDS.to_string(),
            "--observers",
            "1",
        ])
        .spawn()
        .expect("coordinator");
    std::thread::sleep(Duration::from_millis(300));
    let coord_pid = coordinator.id().to_string();
    let watchdog_pid = coord_pid.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(90));
        let _ = Command::new("pkill")
            .args(["-9", "-f", "localhost-barrier"])
            .status();
        let _ = Command::new("kill").args(["-9", &watchdog_pid]).status();
    });
    let extra = [("--coordinator", format!("127.0.0.1:{coord_port}"))];
    let nodes: Vec<Child> = (0..N)
        .map(|id| spawn_node(&spec_s, &peers_s, id, &extra))
        .collect();
    let client = Command::new(env!("CARGO_BIN_EXE_client_driver"))
        .args([
            "--spec",
            &spec_s,
            "--peers",
            &peers_s,
            "--coordinator",
            &format!("127.0.0.1:{coord_port}"),
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("client");
    // a barrier run may legitimately stall; the client's wall clock keeps running meanwhile
    std::thread::sleep(Duration::from_millis(1000));
    let _ = Command::new("kill").args(["-STOP", &coord_pid]).status();
    std::thread::sleep(Duration::from_millis(400));
    let _ = Command::new("kill").args(["-CONT", &coord_pid]).status();
    let records: Vec<Vec<RoundRecord>> = nodes
        .into_iter()
        .map(|c| parse_records(c.wait_with_output().expect("node exit").stdout))
        .collect();
    let client_out =
        String::from_utf8(client.wait_with_output().expect("client exit").stdout).unwrap();
    let _ = coordinator.wait_with_output();
    std::fs::remove_dir_all(&dir).ok();

    let line = client_out
        .lines()
        .rev()
        .find(|l| l.contains("\"commands\""))
        .expect("commands line");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    let client_delivered = v["commands"][0]["delivered_round"]
        .as_u64()
        .expect("client delivered the command");
    let node_round = records[2]
        .iter()
        .find(|r| r.cli_recv > 0)
        .map(|r| r.r)
        .expect("target node received the client command");
    assert_eq!(
        client_delivered, node_round,
        "the pinned target node first received the command in its round {node_round}; the client row says delivered_round={client_delivered} (client wall-clock grid)"
    );
}
