//! Shadow run-config generation (pure; the gen_shadow bin wraps this).

use crate::spec::{NodeRunSpec, ScenarioKind, SyncMode};

const COORD_PORT: u16 = 9100;
const STOP_SLACK_ROUNDS: u64 = 5;

pub fn peers_txt(spec: &NodeRunSpec) -> String {
    (0..spec.n())
        .map(|i| format!("peer{i}:{}\n", spec.listen_port))
        .collect()
}

/// Relative-path form; real runs need the absolute prefixes of [`shadow_yaml_with`].
pub fn shadow_yaml(spec: &NodeRunSpec) -> Result<String, String> {
    shadow_yaml_with(spec, "", "")
}

/// `bin_prefix` prepends the binary path, `run_prefix` the rundir files.
pub fn shadow_yaml_with(
    spec: &NodeRunSpec,
    bin_prefix: &str,
    run_prefix: &str,
) -> Result<String, String> {
    if spec.network.jitter_ms > 0 {
        return Err(
            "jitter shaping is not implemented (Shadow models fixed edge latency); \
             set jitter_ms to 0 or extend deploy.rs first"
                .to_string(),
        );
    }
    let n = spec.n();
    let barrier = spec.sync == SyncMode::Barrier;
    let rounds = spec.max_rounds() as u64;
    // a barrier round also pays one coordinator round trip, and the timer grid starts start_offset_ms late;
    // the built-in 1_gbit_switch (latency_ms 0) is a self-loop of 1 ms, so a hop never costs less than that
    let hop_ms = spec.network.latency_ms.max(1);
    let barrier_ms = if barrier { rounds * 2 * hop_ms } else { 0 };
    let stop_ms = (rounds + STOP_SLACK_ROUNDS) * spec.round_ms + spec.start_offset_ms + barrier_ms;
    let smr = matches!(spec.scenario, ScenarioKind::Smr(_));

    let mut yaml = String::new();
    yaml.push_str("general:\n");
    yaml.push_str(&format!("  stop_time: {stop_ms} ms\n"));
    yaml.push_str("  model_unblocked_syscall_latency: true\n");
    yaml.push_str("network:\n  graph:\n");
    if spec.network.latency_ms == 0 {
        yaml.push_str("    type: 1_gbit_switch\n");
    } else {
        // one attribute per line: Shadow's GML parser rejects the compact single-line form
        yaml.push_str("    type: gml\n    inline: |\n");
        yaml.push_str("      graph [\n");
        yaml.push_str("        directed 0\n");
        yaml.push_str("        node [\n");
        yaml.push_str("          id 0\n");
        yaml.push_str("          host_bandwidth_up \"1 Gbit\"\n");
        yaml.push_str("          host_bandwidth_down \"1 Gbit\"\n");
        yaml.push_str("        ]\n");
        yaml.push_str("        edge [\n");
        yaml.push_str("          source 0\n");
        yaml.push_str("          target 0\n");
        yaml.push_str(&format!(
            "          latency \"{} ms\"\n",
            spec.network.latency_ms
        ));
        yaml.push_str("          packet_loss 0.0\n");
        yaml.push_str("        ]\n");
        yaml.push_str("      ]\n");
    }
    yaml.push_str("hosts:\n");

    let coord_arg = if barrier {
        format!(" --coordinator coordinator:{COORD_PORT}")
    } else {
        String::new()
    };
    for i in 0..n {
        yaml.push_str(&format!("  peer{i}:\n"));
        yaml.push_str("    network_node_id: 0\n    processes:\n");
        yaml.push_str(&format!("    - path: {bin_prefix}node\n"));
        yaml.push_str(&format!(
            "      args: --spec {run_prefix}spec.json --peers {run_prefix}peers.txt --node-id {i}{coord_arg}\n"
        ));
        yaml.push_str("      expected_final_state:\n        exited: 0\n");
    }
    if barrier {
        yaml.push_str("  coordinator:\n    network_node_id: 0\n    processes:\n");
        yaml.push_str(&format!("    - path: {bin_prefix}coordinator\n"));
        let observers = if smr { " --observers 1" } else { "" };
        yaml.push_str(&format!(
            "      args: --port {COORD_PORT} --n {n} --rounds {}{observers}\n",
            spec.max_rounds()
        ));
        yaml.push_str("      expected_final_state:\n        exited: 0\n");
    }
    if smr {
        yaml.push_str("  client:\n    network_node_id: 0\n    processes:\n");
        yaml.push_str(&format!("    - path: {bin_prefix}client_driver\n"));
        yaml.push_str(&format!(
            "      args: --spec {run_prefix}spec.json --peers {run_prefix}peers.txt{coord_arg}\n"
        ));
        yaml.push_str("      expected_final_state:\n        exited: 0\n");
    }
    Ok(yaml)
}
