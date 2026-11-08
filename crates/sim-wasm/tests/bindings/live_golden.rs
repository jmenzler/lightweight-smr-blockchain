//! Frozen digests of every live-session output, so a speed change to the step path can be shown
//! to leave each produced value unchanged. Digests hash the parsed, key-sorted form.

use crate::batch_view::without_live_only;
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
    let kind = serde_json::from_str::<Value>(proto).unwrap()["kind"]
        .as_str()
        .unwrap()
        .to_string();
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
        // the report column stays frozen: everything but the live-only T_E must be unchanged
        let report = serde_json::from_str::<Value>(&live.report_json()).unwrap();
        d.report.feed(&without_live_only(report, &kind).to_string());
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
/// Compact and recovery deltas carry the live-only T_E; the report column digests it cleared.
const GOLDEN: &[(&str, u64, [u64; 5])] = &[
    (
        "extended",
        1,
        [
            0x4904dd5bba1927de,
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
            0xf73cc0b6becf0053,
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
            0x0afb0a7bd3b512e5,
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
            0x8a1e5fc22ade10ed,
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
            0x0c17711d05639dec,
            0xe260592b2e66d241,
            0x84c79db8add86474,
            0xfc74bb8b33108760,
            0xcc8e826a8c7e3f74,
        ],
    ),
    (
        "compact",
        7,
        [
            0xb45ff6664d504712,
            0xf66992f0872fe206,
            0x1bce44e8e6d1ebe9,
            0xd3bcca9957a893d9,
            0xb74054142f1de56f,
        ],
    ),
    (
        "compact",
        23,
        [
            0x287cbaae54a86594,
            0x379316a9535c6533,
            0xcc25fa1a7526affc,
            0xb9d1c935e9a4acd6,
            0xb74054142f1de56f,
        ],
    ),
    (
        "compact",
        42,
        [
            0x4b6b44e28f9a7992,
            0x3dc06969298f1801,
            0xc5bf42a4de134dec,
            0xc9ef767c46514581,
            0xb74054142f1de56f,
        ],
    ),
    (
        "recovery",
        1,
        [
            0x1441e6a4c6845b20,
            0x0f0f017e828fe0f5,
            0xd7f566f9d35eae58,
            0xd167173118d12c2d,
            0x5a674aab0c341d33,
        ],
    ),
    (
        "recovery",
        7,
        [
            0xc50e45e877dde28d,
            0xe631ec8c526f26af,
            0x23523a2b68f8d17c,
            0xd9a891b7675d2a84,
            0xb40ad97bd55a04a3,
        ],
    ),
    (
        "recovery",
        23,
        [
            0x829025c5205599ed,
            0x4f0439aea9d0f056,
            0xb15342c671b69c5e,
            0xd9a891b7675d2a84,
            0xb40ad97bd55a04a3,
        ],
    ),
    (
        "recovery",
        42,
        [
            0x191a8e0a7183e26e,
            0x41ed866c14905c65,
            0x5742ec2932d2d447,
            0xf00eee6eb2bf4c04,
            0xdb64e357be6f4733,
        ],
    ),
    (
        "recovery_certs",
        1,
        [
            0xc11b667856e6faf4,
            0x5967e42778b155e5,
            0xe6b4bdc35114b43e,
            0xed3a66c42504a977,
            0x6dadcb3cf0ff28f2,
        ],
    ),
    (
        "recovery_certs",
        7,
        [
            0xd6e6b6904e3a39e8,
            0x032af83fae7dc47d,
            0x5bef3c6443c1766c,
            0xebdc7ecfca0a05cd,
            0x9cd654c6d739db57,
        ],
    ),
    (
        "recovery_certs",
        23,
        [
            0x8834c8d92e7717fe,
            0xbeb30e33af9af3b0,
            0xc5b5dd1fbbe5d473,
            0x7a748f0a6b732c25,
            0xb4d09eb9dd5d097f,
        ],
    ),
    (
        "recovery_certs",
        42,
        [
            0xc5e90c9e1a91f58e,
            0xb9a8b3e7080db5f4,
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
    (1, 0x241ffadbf62aabc6),
    (7, 0xc0aff575674b8695),
    (23, 0xb72cd1e968b915c7),
    (42, 0x28e85d5d4b4299ca),
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
