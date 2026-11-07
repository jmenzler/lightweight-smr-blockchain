//! The executed-prefix digest is ONE encoding shared by `crates/network-node` and `crates/sim`.
//! Pinned digests from an independent FNV-1a computation catch any change to that encoding, which a
//! comparison of the shared function with itself cannot.

use protocol::compact::{ClientCommand, Entry};

fn cmd(client: u32, sn: u64, op: u64) -> Entry {
    Entry::Cmd(ClientCommand { client, sn, op })
}

fn cases() -> Vec<(&'static str, Vec<Entry>)> {
    vec![
        ("empty", vec![]),
        ("cmd", vec![cmd(1, 1, 7)]),
        ("null", vec![Entry::Null { client: 1, sn: 1 }]),
        ("nop", vec![Entry::Nop(5)]),
        (
            "mixed",
            vec![
                cmd(1, 1, 7),
                Entry::Null { client: 2, sn: 1 },
                Entry::Nop(9),
            ],
        ),
    ]
}

/// FNV-1a 64 over the documented byte encoding, computed outside the code under test.
const PINNED: [(&str, u64); 5] = [
    ("empty", 0xcbf2_9ce4_8422_2325),
    ("cmd", 0x2ca8_99e8_2cd3_3d98),
    ("null", 0xc2ed_79bc_664f_82fc),
    ("nop", 0x71f3_49c8_1719_a340),
    ("mixed", 0x67a5_09e1_d626_01d7),
];

#[test]
fn every_entry_variant_hashes_to_its_independently_computed_digest() {
    for (name, entries) in cases() {
        let want = PINNED.iter().find(|(n, _)| *n == name).expect("pinned").1;
        assert_eq!(
            network_node::digest::fnv64(&entries),
            want,
            "{name}: the executed-prefix digest encoding changed"
        );
    }
}

#[test]
fn the_aged_log_and_op_digests_are_pinned_too() {
    let timed = [protocol::compact::Timed {
        entry: cmd(1, 1, 7),
        round: 3,
    }];
    assert_eq!(
        network_node::digest::fnv_timed(timed.iter()),
        0x5b69_b4f4_68be_befb
    );
    assert_eq!(
        network_node::digest::fnv_ops([7u64, 9].into_iter()),
        0xb1f6_ddb2_8a29_b6cb
    );
}

#[test]
fn folding_entry_by_entry_matches_hashing_the_whole_slice() {
    for (name, entries) in cases() {
        let folded = entries
            .iter()
            .fold(sim::digest::FNV_BASIS, sim::digest::fold_entry);
        assert_eq!(
            folded,
            sim::digest::fnv64(&entries),
            "{name}: incremental fold diverges from the slice digest"
        );
    }
}
