//! Deterministic injection-schedule generator for an SMR spec's `injections`.
//! Targets stay unset (a fresh uniform server per attempt) unless `--pin-targets` is given.

use network_node::cli::arg;

fn die(msg: &str) -> ! {
    network_node::cli::die("gen_schedule", msg)
}

fn main() {
    let seed: u64 = arg("--seed")
        .unwrap_or_else(|| die("--seed required"))
        .parse()
        .unwrap_or_else(|_| die("--seed must be a u64"));
    let rounds: u64 = arg("--rounds")
        .unwrap_or_else(|| die("--rounds required"))
        .parse()
        .unwrap_or_else(|_| die("--rounds must be a u64"));
    let pmf_json = arg("--pmf").unwrap_or_else(|| die("--pmf required"));
    let pmf: Vec<f64> = serde_json::from_str(&pmf_json)
        .unwrap_or_else(|e| die(&format!("--pmf must be a JSON array of numbers: {e}")));
    let clients: u32 = arg("--clients")
        .unwrap_or_else(|| die("--clients required"))
        .parse()
        .unwrap_or_else(|_| die("--clients must be a u32"));
    let per_client: u32 = arg("--per-client")
        .unwrap_or_else(|| die("--per-client required"))
        .parse()
        .unwrap_or_else(|_| die("--per-client must be a u32"));
    let n: u32 = arg("--n")
        .unwrap_or_else(|| die("--n required"))
        .parse()
        .unwrap_or_else(|_| die("--n must be a u32"));
    let pin_targets = std::env::args().any(|a| a == "--pin-targets");
    let out = arg("--out").unwrap_or_else(|| die("--out required"));

    let schedule =
        network_node::schedule::generate(seed, rounds, &pmf, clients, per_client, n, pin_targets)
            .unwrap_or_else(|e| die(&e));
    let json = serde_json::to_string(&schedule).expect("schedule serializes");
    std::fs::write(&out, json).unwrap_or_else(|e| die(&format!("write {out}: {e}")));

    let meta = network_node::schedule::Meta {
        seed,
        pmf,
        rounds,
        clients,
        per_client,
        n,
        pin_targets,
    };
    let meta_path = format!("{out}.meta.json");
    let meta_json = serde_json::to_string(&meta).expect("meta serializes");
    std::fs::write(&meta_path, meta_json)
        .unwrap_or_else(|e| die(&format!("write {meta_path}: {e}")));

    println!("{out}");
}
