//! Real-socket check that a lost message reaches the validity gates.

use network_node::collect::aggregate;
use network_node::record::RoundRecord;
use network_node::spec::NodeRunSpec;
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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

// Loss exists only through blocking (p. 4), so unblocked transport loss must make the run non-clean.
#[test]
#[ignore = "slow: cargo test --release -- --ignored"]
fn unreachable_peer_transport_loss_makes_gates_unclean() {
    const N: usize = 3;
    const ROUNDS: usize = 8;
    let dir = std::env::temp_dir().join(format!("localhost-loss-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = format!(
        r#"{{"scenario":{{"median":{{"n":{N},"seed":11,"k":6,"ell":3,
             "init":{{"kind":"distinct"}},"max_rounds":{ROUNDS}}}}},
             "sync":"timer","round_ms":150}}"#
    );
    let spec_path = dir.join("spec.json");
    std::fs::write(&spec_path, &spec).unwrap();
    let base = free_base(0);
    let peers: String = (0..N)
        .map(|i| format!("127.0.0.1:{}\n", base + i as u16))
        .collect();
    let peers_path = dir.join("peers.txt");
    std::fs::write(&peers_path, peers).unwrap();
    let (spec_s, peers_s) = (
        spec_path.to_string_lossy().into_owned(),
        peers_path.to_string_lossy().into_owned(),
    );
    let start_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 600;
    let extra = [("--start-at-ms", start_at.to_string())];
    // node 2 never starts: every request addressed to it fails at connect
    let children: Vec<Child> = (0..2)
        .map(|id| spawn_node(&spec_s, &peers_s, id, &extra))
        .collect();
    let records: Vec<Vec<RoundRecord>> = children
        .into_iter()
        .map(|c| parse_records(c.wait_with_output().expect("node exit").stdout))
        .collect();
    std::fs::remove_dir_all(&dir).ok();

    let sent: u32 = records.iter().flatten().map(|r| r.req_sent).sum();
    let received: u32 = records.iter().flatten().map(|r| r.rep_recv).sum();
    let blocked = records.iter().flatten().any(|r| r.blocked);
    assert!(
        !blocked && sent > received,
        "no loss observed: sent={sent} received={received}"
    );

    let nspec = NodeRunSpec::from_json(&spec).unwrap();
    let mut out = aggregate(&records, &nspec).expect("aggregate");
    // timing gates measure the machine; isolate the transport-loss verdict from them
    out.gates.late_rep = 0;
    out.gates.late_app = 0;
    assert!(out.gates.transport_err > 0, "{:?}", out.gates);
    assert!(
        !out.gates.clean(),
        "{} of {sent} unblocked pull requests got no reply (peer unreachable), yet gates are clean: {:?}",
        sent - received,
        out.gates
    );
}
