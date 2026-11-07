//! Frozen digests of every live-session output, so a speed change to the step path can be shown
//! to leave each produced value unchanged. Digests hash the parsed, key-sorted form.

use serde_json::Value;
use sim_wasm::LiveSmr;

const PROTOS: [(&str, &str); 4] = [
    ("extended", r#"{"kind":"extended"}"#),
    ("compact", r#"{"kind":"compact","t_commit_rounds":6}"#),
    (
        "recovery",
        r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":false}"#,
    ),
    (
        "recovery_certs",
        r#"{"kind":"recovery","t_window_rounds":10,"resend_until_acked":true}"#,
    ),
];
const SEEDS: [u64; 4] = [1, 7, 23, 42];
const ROUNDS: usize = 60;

fn spec(proto: &str, seed: u64) -> String {
    format!(
        r#"{{"n":32,"seed":{seed},"k":6,"ell":3,"sigma":1.5,"proto":{proto},
            "injections":[{{"round":2,"client":1,"op":7}},{{"round":3,"client":2,"op":8}}],
            "max_rounds":400,
            "schedule":{{"kind":"fresh_per_round","fraction":0.1}},
            "traffic":{{"kind":"pmf","arrivals_pmf":[0.3,0.4,0.3]}}}}"#
    )
}

fn canonical(json: &str) -> String {
    serde_json::from_str::<Value>(json)
        .expect("output is json")
        .to_string()
}

#[derive(Default)]
struct Digest(u64);

impl Digest {
    fn feed(&mut self, json: &str) {
        if self.0 == 0 {
            self.0 = 0xcbf2_9ce4_8422_2325;
        }
        for b in canonical(json).bytes().chain([0xff]) {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

#[derive(Default)]
struct Digests {
    status: Digest,
    delta: Digest,
    report: Digest,
    certs: Digest,
    probes: Digest,
}

fn mid_window_adoptions(status: &str) -> bool {
    let v: Value = serde_json::from_str(status).unwrap();
    let round = v["round"].as_u64().unwrap();
    !round.is_multiple_of(10)
        && v["rec"]["adoptions"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
}

fn attested(certs: &str) -> bool {
    let v: Value = serde_json::from_str(certs).unwrap();
    v["servers"]
        .as_array()
        .is_some_and(|s| s.iter().any(|row| row["m"].as_u64() > Some(0)))
}

/// Also returns whether a checkpoint was adopted off a window boundary and whether any server
/// attested a command, so the cert-reuse path is shown to run on changed and unchanged rounds.
fn run(proto: &str, seed: u64) -> (Digests, bool, bool) {
    let mut live = LiveSmr::new(&spec(proto, seed)).unwrap();
    let mut d = Digests::default();
    let mut adopted_mid_window = false;
    let mut any_attested = false;
    for r in 1..=ROUNDS {
        match r {
            15 => d.probes.feed(&live.inject(3, 99, None)),
            20 => d.probes.feed(&live.set_block(4, true)),
            30 => d.probes.feed(&live.set_block(4, false)),
            25 | 45 => {
                d.probes.feed(&live.capture_stale(1));
                d.probes.feed(&live.capture_stale(2));
            }
            _ => {}
        }
        let status = live.step(0.1);
        adopted_mid_window |= mid_window_adoptions(&status);
        d.status.feed(&status);
        d.delta.feed(&live.report_delta_json());
        d.report.feed(&live.report_json());
        let certs = live.certs_json();
        any_attested |= attested(&certs);
        d.certs.feed(&certs);
        if r % 10 == 0 || r % 10 == 1 {
            d.probes.feed(&live.verify_json());
        }
    }
    (d, adopted_mid_window, any_attested)
}

/// (proto, seed) -> [status, delta, report, certs, probes]
const GOLDEN: &[(&str, u64, [u64; 5])] = &[
    (
        "extended",
        1,
        [
            0x2e493b0d5416d4ce,
            0xa319b8f296c9500e,
            0x3ac6ed8c69cda2fc,
            0x8ca4a4645c95ae45,
            0x31e02b0a5ee23921,
        ],
    ),
    (
        "extended",
        7,
        [
            0xa884eb69f18f080d,
            0x43be284e233b4fed,
            0xf2f2413d0bd653b5,
            0x8ca4a4645c95ae45,
            0x31e02b0a5ee23921,
        ],
    ),
    (
        "extended",
        23,
        [
            0xbb3bfe93cb89ca74,
            0x8c35465543a1db56,
            0x2d178cb4736cf320,
            0x8ca4a4645c95ae45,
            0x31e02b0a5ee23921,
        ],
    ),
    (
        "extended",
        42,
        [
            0x00a04524610a94a8,
            0xfe6a6a7844e06729,
            0xe75e1107bf3ee41c,
            0x8ca4a4645c95ae45,
            0x31e02b0a5ee23921,
        ],
    ),
    (
        "compact",
        1,
        [
            0x82dd857997a31bad,
            0xb8fc3a71329186b7,
            0x84c79db8add86474,
            0xfc74bb8b33108760,
            0xcc8e826a8c7e3f74,
        ],
    ),
    (
        "compact",
        7,
        [
            0xd9d60a799d203c0f,
            0x88964054ac9090dd,
            0x1bce44e8e6d1ebe9,
            0xd3bcca9957a893d9,
            0xb74054142f1de56f,
        ],
    ),
    (
        "compact",
        23,
        [
            0xe6981e91eb33d637,
            0x000fb2ba7d9aa028,
            0xcc25fa1a7526affc,
            0xb9d1c935e9a4acd6,
            0xb74054142f1de56f,
        ],
    ),
    (
        "compact",
        42,
        [
            0x216ca688401b1848,
            0x80d9d749991c2f4f,
            0xc5bf42a4de134dec,
            0xc9ef767c46514581,
            0xb74054142f1de56f,
        ],
    ),
    (
        "recovery",
        1,
        [
            0x14b3d863822925f4,
            0xca4482669d675f6c,
            0xd7f566f9d35eae58,
            0xd167173118d12c2d,
            0x5a674aab0c341d33,
        ],
    ),
    (
        "recovery",
        7,
        [
            0x6325ad9b3d6231e1,
            0xe54f3f1a242f929a,
            0x23523a2b68f8d17c,
            0xd9a891b7675d2a84,
            0xb40ad97bd55a04a3,
        ],
    ),
    (
        "recovery",
        23,
        [
            0xe5c082732d4933ad,
            0xb76f781580a59316,
            0xb15342c671b69c5e,
            0xd9a891b7675d2a84,
            0xb40ad97bd55a04a3,
        ],
    ),
    (
        "recovery",
        42,
        [
            0xa605e9307cda8bde,
            0xecb090f4d6b22669,
            0x5742ec2932d2d447,
            0xf00eee6eb2bf4c04,
            0xdb64e357be6f4733,
        ],
    ),
    (
        "recovery_certs",
        1,
        [
            0x799787425b72ab58,
            0x92eac0393f76d981,
            0xe6b4bdc35114b43e,
            0xed3a66c42504a977,
            0x6dadcb3cf0ff28f2,
        ],
    ),
    (
        "recovery_certs",
        7,
        [
            0x48d32c71b371499b,
            0x8d6f3471699bc7be,
            0x5bef3c6443c1766c,
            0xebdc7ecfca0a05cd,
            0x9cd654c6d739db57,
        ],
    ),
    (
        "recovery_certs",
        23,
        [
            0x5e10af73d507a43b,
            0x5254e288e18f91ad,
            0xc5b5dd1fbbe5d473,
            0x7a748f0a6b732c25,
            0xb4d09eb9dd5d097f,
        ],
    ),
    (
        "recovery_certs",
        42,
        [
            0xf857a219437cea26,
            0x74fa91c096bbaed6,
            0xa4a479bdc6119fee,
            0xc84992f6a2bc9145,
            0xa3a1e0182cd3eb64,
        ],
    ),
];

#[test]
fn live_outputs_match_their_frozen_digests() {
    let mut got = Vec::new();
    let mut adoptions = false;
    let mut attestations = false;
    for (name, proto) in PROTOS {
        for seed in SEEDS {
            let (d, adopted, attested) = run(proto, seed);
            if name == "recovery_certs" {
                adoptions |= adopted;
                attestations |= attested;
            }
            got.push((
                name,
                seed,
                [d.status.0, d.delta.0, d.report.0, d.certs.0, d.probes.0],
            ));
        }
    }
    for (name, seed, h) in &got {
        println!(
            "    (\"{name}\", {seed}, [{:#018x}, {:#018x}, {:#018x}, {:#018x}, {:#018x}]),",
            h[0], h[1], h[2], h[3], h[4]
        );
    }
    assert!(
        adoptions,
        "a cert-carrying recovery session must adopt a peer checkpoint mid-window"
    );
    assert!(
        attestations,
        "a cert-carrying recovery session must attest a command"
    );
    assert_eq!(got.len(), GOLDEN.len());
    for (g, want) in got.iter().zip(GOLDEN) {
        assert_eq!((g.0, g.1, g.2), (want.0, want.1, want.2));
    }
}

fn has_chain(certs: &str) -> bool {
    let v: Value = serde_json::from_str(certs).unwrap();
    v["clients"].as_array().is_some_and(|cs| {
        cs.iter().any(|c| {
            c["certs"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|r| !r["chain_len"].is_null()))
        })
    })
}

fn all_acked(certs: &str, client: u64) -> bool {
    let v: Value = serde_json::from_str(certs).unwrap();
    v["clients"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["client"].as_u64() == Some(client)))
        .is_none_or(|c| {
            c["certs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["committed"] == true)
        })
}

const REPEAT_ROUNDS: usize = 150;

fn repeat_spec(seed: u64) -> String {
    format!(
        r#"{{"n":32,"seed":{seed},"k":6,"ell":3,"sigma":5.0,
            "proto":{{"kind":"recovery","t_window_rounds":10,"resend_until_acked":true}},
            "injections":[],"max_rounds":400,
            "schedule":{{"kind":"fresh_per_round","fraction":0.0}}}}"#
    )
}

/// One client re-issuing as soon as its last command is acked, so acks carry real notifications.
fn run_repeat_client(seed: u64) -> (u64, bool) {
    let mut live = LiveSmr::new(&repeat_spec(seed)).unwrap();
    let mut d = Digest::default();
    let mut chained = false;
    let mut certs = live.certs_json();
    for r in 1..=REPEAT_ROUNDS {
        if all_acked(&certs, 9) {
            d.feed(&live.inject(9, 1000 + r as u64, None));
        }
        let status = live.step(0.05);
        d.feed(&status);
        certs = live.certs_json();
        d.feed(&certs);
        chained |= has_chain(&certs);
    }
    (d.0, chained)
}

const GOLDEN_REPEAT_CLIENT: [(u64, u64); 4] = [
    (1, 0x58a8e2c7ecd95027),
    (7, 0x73cfb47ba6516ace),
    (23, 0x55902ccf14200e98),
    (42, 0x55ef4bfc6bcee707),
];

#[test]
fn repeat_client_ack_notifications_match_their_frozen_digests() {
    let got: Vec<(u64, u64)> = SEEDS
        .iter()
        .map(|&seed| {
            let (digest, chained) = run_repeat_client(seed);
            assert!(chained, "seed {seed}: no ack delivered a chain");
            (seed, digest)
        })
        .collect();
    println!("    {got:#018x?}");
    assert_eq!(got, GOLDEN_REPEAT_CLIENT);
}
