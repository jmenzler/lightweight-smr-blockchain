use protocol::certificates::{
    Certificate, ClientChains, Hash, MmrForest, Notification, ServerCertState, leaf_hash,
};
use protocol::compact::{ClientCommand, Entry};
use sha2::{Digest, Sha256};

fn cmd(client: u32, sn: u64, op: u64) -> Entry {
    Entry::Cmd(ClientCommand { client, sn, op })
}

/// Reference fold: climb from the leaf using the chain, sibling side taken
/// from the position bits (interior = h(0x01‖left‖right)).
fn ref_fold(leaf: Hash, pos: u64, chain: &[Hash]) -> Hash {
    let mut h = leaf;
    for (i, sib) in chain.iter().enumerate() {
        let mut s = Sha256::new();
        s.update([0x01]);
        if (pos >> i) & 1 == 1 {
            s.update(sib);
            s.update(h);
        } else {
            s.update(h);
            s.update(sib);
        }
        h = s.finalize().into();
    }
    h
}

/// Root of the forest tree covering `pos` (trees cover consecutive ranges,
/// tallest first, §5 p. 27).
fn covering_root(forest: &MmrForest, pos: u64) -> Hash {
    let mut start = 0u64;
    for (height, root) in forest.peak_heights().into_iter().zip(forest.roots()) {
        let size = 1u64 << height;
        if pos < start + size {
            return root;
        }
        start += size;
    }
    panic!("pos {pos} beyond forest");
}

/// Heights of the set bits of m, descending — the paper's unique decomposition
/// m = 2^{i₁} + … + 2^{i_a} with i₁ > … > i_a (§5 p. 27).
fn binary_decomposition(m: u64) -> Vec<u32> {
    (0..64).rev().filter(|i| (m >> i) & 1 == 1).collect()
}

#[test]
fn server_keeps_the_last_two_commands_per_client_and_notifies_with_the_previous_chain() {
    let mut server = ServerCertState::new();
    server.append(&Entry::Nop(0));
    server.append(&cmd(7, 1, 100));
    // Only one command of client 7 committed: no x₁ yet, nothing to attach.
    assert!(server.notification(7).is_none());

    server.append(&Entry::Nop(1));
    server.append(&cmd(7, 2, 101));
    // Window is (x₁ = sn 1 at pos 1, x₂ = sn 2 at pos 3); the ack for x₂
    // carries hc(x₁) and p(x₁) (§5 p. 28).
    let note = server.notification(7).expect("x1 exists");
    assert_eq!(note.prev_pos, 1);
    assert_eq!(note.prev_sn, 1);

    server.append(&cmd(7, 3, 102));
    // x₁ rotated out: window is now (sn 2 at pos 3, sn 3 at pos 4).
    let note = server.notification(7).expect("window rotated");
    assert_eq!(note.prev_pos, 3);
    assert_eq!(note.prev_sn, 2);

    // A null command still advances the client's window (identity kept).
    server.append(&Entry::Null { client: 7, sn: 4 });
    let note = server.notification(7).expect("null rotates too");
    assert_eq!(note.prev_pos, 4);
    assert_eq!(note.prev_sn, 3);

    // Unknown client: nothing stored.
    assert!(server.notification(99).is_none());
}

#[test]
fn client_keeps_the_longer_chain_and_its_next_sn_is_k_plus_2() {
    let h = [9u8; 32];
    let note = |pos, sn, len| Notification {
        prev_chain: vec![h; len],
        prev_pos: pos,
        prev_sn: sn,
    };

    let mut client = ClientChains::new(7);
    client.record_issue(&cmd(7, 1, 100));
    client.on_commit_ack(1, None);
    client.record_issue(&cmd(7, 2, 101));
    client.on_commit_ack(2, Some(note(1, 1, 1)));
    // k = 1 chains received → sn = k + 2 (§5 p. 28).
    assert_eq!(client.next_sn(), 3);
    assert_eq!(client.chain_for(1).unwrap().1.len(), 1);

    // Re-acks may carry chains of different length for the same command:
    // the longer one wins, a shorter one never regresses (§5 p. 28).
    client.on_commit_ack(2, Some(note(1, 1, 3)));
    assert_eq!(client.chain_for(1).unwrap().1.len(), 3);
    client.on_commit_ack(2, Some(note(1, 1, 2)));
    assert_eq!(client.chain_for(1).unwrap().1.len(), 3);

    // No chain yet for the newest committed command.
    assert!(client.chain_for(2).is_none());
}

#[test]
fn every_committed_command_verifies_at_any_useful_server() {
    // Client 7's commands interleaved with foreign traffic; after every ack,
    // certificates for ALL its committed commands verify at BOTH servers
    // (Thm 6). Re-acking after every append keeps the stitched chains at the covering peak, so
    // this sweep exercises case (a) and the trivial x = x_{k+1} check; case (b) is pinned by
    // `case_b_decides_when_the_chain_stops_short_of_the_covering_peak`.
    let mut s1 = ServerCertState::new();
    let mut s2 = ServerCertState::new();
    let mut client = ClientChains::new(7);
    let mut sn = 0u64;
    for i in 0..48u64 {
        let entry = match i % 3 {
            0 => {
                sn += 1;
                let e = cmd(7, sn, 700 + sn);
                client.record_issue(&e);
                e
            }
            1 => Entry::Nop(i),
            _ => cmd(3, i / 3 + 1, 300 + i),
        };
        s1.append(&entry);
        s2.append(&entry);
        if sn > 0 {
            // Resubmission ack from one server, reply-time snapshot (§5 p. 28).
            client.on_commit_ack(sn, s1.notification(7));
        }
        for want in 1..=sn {
            let cert = client
                .build_certificate(want)
                .unwrap_or_else(|| panic!("no cert for sn {want} at m {}", i + 1));
            assert!(s1.verify(7, &cert), "sn {want} at m {} on s1", i + 1);
            assert!(s2.verify(7, &cert), "sn {want} at m {} on s2", i + 1);
        }
    }
}

/// Commits `count` commands of client 7 (interleaved with foreign traffic)
/// into a server, acking after each; returns (server, client).
fn committed_run(count: u64) -> (ServerCertState, ClientChains) {
    let mut server = ServerCertState::new();
    let mut client = ClientChains::new(7);
    for sn in 1..=count {
        server.append(&Entry::Nop(1000 + sn));
        let e = cmd(7, sn, 700 + sn);
        client.record_issue(&e);
        server.append(&e);
        client.on_commit_ack(sn, server.notification(7));
    }
    (server, client)
}

#[test]
fn tampered_certificates_are_rejected() {
    let (server, client) = committed_run(6);
    let cert = client.build_certificate(2).unwrap();
    assert!(server.verify(7, &cert), "untampered baseline");
    let protocol::certificates::Certificate::Chained { entry, pos, chain } = cert else {
        panic!("sn 2 must be a chained certificate")
    };
    let Entry::Cmd(c) = entry else { panic!() };

    let wrong_pos = protocol::certificates::Certificate::Chained {
        entry: Entry::Cmd(c),
        pos: pos + 1,
        chain: chain.clone(),
    };
    assert!(!server.verify(7, &wrong_pos));

    let altered = protocol::certificates::Certificate::Chained {
        entry: Entry::Cmd(ClientCommand { op: c.op + 1, ..c }),
        pos,
        chain: chain.clone(),
    };
    assert!(!server.verify(7, &altered));

    let truncated = protocol::certificates::Certificate::Chained {
        entry: Entry::Cmd(c),
        pos,
        chain: chain[..chain.len() - 1].to_vec(),
    };
    assert!(!server.verify(7, &truncated));

    let mut corrupted = chain.clone();
    corrupted[0][0] ^= 0xff;
    let bad_link = protocol::certificates::Certificate::Chained {
        entry: Entry::Cmd(c),
        pos,
        chain: corrupted,
    };
    assert!(!server.verify(7, &bad_link));

    assert!(chain.len() >= 2, "need ≥2 links to test reordering");
    let mut swapped = chain.clone();
    swapped.swap(0, 1);
    let reordered = protocol::certificates::Certificate::Chained {
        entry: Entry::Cmd(c),
        pos,
        chain: swapped,
    };
    assert!(!server.verify(7, &reordered));
}

#[test]
fn newest_certificate_survives_one_further_commit_and_dies_on_eviction() {
    // §5 p. 29 says stale certificates "might" fail — never that one newer
    // commit fails: the last-two window still holds x_{k+1} after ONE further
    // commit. Eviction (≥ 2 further commits) makes rejection deterministic
    // for the bare-newest certificate.
    let (mut server, client) = committed_run(4);
    let stale = client.build_certificate(4).unwrap();
    assert!(matches!(
        stale,
        protocol::certificates::Certificate::Newest { .. }
    ));
    assert!(server.verify(7, &stale), "fresh");

    server.append(&cmd(7, 5, 705));
    assert!(
        server.verify(7, &stale),
        "one further commit: still in the window"
    );

    server.append(&cmd(7, 6, 706));
    assert!(!server.verify(7, &stale), "two further commits: evicted");
}

#[test]
fn losing_one_chain_forfeits_only_that_command() {
    // §5 p. 27: loss of the chain for x forfeits x's certification only. This layout (commands at
    // 1, 3, 5, 7, 9) lets sn 3's chain extend sn 1; see
    // `a_lost_chain_can_stall_the_extension_of_an_earlier_certificate` for the other case.
    let mut server = ServerCertState::new();
    let mut intact = ClientChains::new(7);
    let mut lossy = ClientChains::new(7);
    for sn in 1..=5u64 {
        server.append(&Entry::Nop(1000 + sn));
        let e = cmd(7, sn, 700 + sn);
        intact.record_issue(&e);
        lossy.record_issue(&e);
        server.append(&e);
        let note = server.notification(7);
        intact.on_commit_ack(sn, note.clone());
        // The lossy client never received (or lost) the chain for sn 2.
        lossy.on_commit_ack(sn, note.filter(|n| n.prev_sn != 2));
    }
    assert!(lossy.build_certificate(2).is_none(), "sn 2 forfeit");
    for sn in [1, 3, 4, 5] {
        let cert = lossy
            .build_certificate(sn)
            .unwrap_or_else(|| panic!("sn {sn}"));
        assert!(server.verify(7, &cert), "sn {sn} unaffected");
    }
}

#[test]
fn stored_chains_always_reach_their_trees_root() {
    // v2 p. 28: hc(x) = "the sibling hashes of x and all ancestors of x in
    // its Merkle hash tree" — after EVERY append, every stored chain must
    // fold from its leaf to the root of its covering tree.
    let mut server = ServerCertState::new();
    for i in 0..40u64 {
        let entry = match i % 3 {
            0 => Entry::Nop(i),
            1 => cmd(1, i / 3 + 1, 200 + i),
            _ => cmd(2, i / 3 + 1, 300 + i),
        };
        server.append(&entry);
        for client in [1u32, 2] {
            let Some((prev, last)) = server.last_two(client) else {
                continue;
            };
            for sc in prev.iter().copied().chain([last]) {
                assert_eq!(
                    ref_fold(sc.leaf, sc.pos, &sc.chain),
                    covering_root(server.forest(), sc.pos),
                    "m = {}, client {client}, pos {}",
                    i + 1,
                    sc.pos
                );
            }
        }
    }
}

#[test]
fn roots_pin_rfc6962_domain_separated_sha256() {
    // Hand-computed vector: two leaves → root = H(0x01 ‖ H(0x00‖enc(a)) ‖ H(0x00‖enc(b))).
    let (a, b) = (Entry::Nop(0), Entry::Nop(1));
    let mut forest = MmrForest::new();
    forest.append(leaf_hash(&a));
    forest.append(leaf_hash(&b));
    let expect_leaf = |e: &Entry| -> Hash {
        let Entry::Nop(op) = e else { panic!() };
        let mut s = Sha256::new();
        s.update([0x00, 0x03]);
        s.update(op.to_le_bytes());
        s.finalize().into()
    };
    let mut s = Sha256::new();
    s.update([0x01]);
    s.update(expect_leaf(&a));
    s.update(expect_leaf(&b));
    let root: Hash = s.finalize().into();
    assert_eq!(forest.roots(), vec![root]);
}

/// Literal leaf bytes: 0x00 ‖ tag ‖ little-endian fields, spelled out byte by byte.
fn literal_leaf_bytes(entry: &Entry) -> Vec<u8> {
    match entry {
        Entry::Cmd(c) => {
            let mut b = vec![0x00, 0x01];
            b.extend(c.client.to_le_bytes());
            b.extend(c.sn.to_le_bytes());
            b.extend(c.op.to_le_bytes());
            b
        }
        Entry::Null { client, sn } => {
            let mut b = vec![0x00, 0x02];
            b.extend(client.to_le_bytes());
            b.extend(sn.to_le_bytes());
            b
        }
        Entry::Nop(op) => {
            let mut b = vec![0x00, 0x03];
            b.extend(op.to_le_bytes());
            b
        }
    }
}

/// Root of a complete binary tree over `leaves` (a power of two), by plain recursion.
fn complete_tree_root(leaves: &[Entry]) -> Hash {
    if let [only] = leaves {
        return Sha256::digest(literal_leaf_bytes(only)).into();
    }
    let (left, right) = leaves.split_at(leaves.len() / 2);
    let mut s = Sha256::new();
    s.update([0x01]);
    s.update(complete_tree_root(left));
    s.update(complete_tree_root(right));
    s.finalize().into()
}

#[test]
fn cmd_and_null_leaf_encodings_are_pinned_byte_for_byte() {
    // Fixed bytes written out by hand: 0x00, tag 0x01, client u32, sn u64, op u64, all
    // little-endian. A change to any field order, width or tag moves the hash.
    let cmd_bytes: Vec<u8> = vec![
        0x00, 0x01, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0,
    ];
    assert_eq!(literal_leaf_bytes(&cmd(1, 2, 3)), cmd_bytes);
    assert_eq!(
        leaf_hash(&cmd(1, 2, 3)),
        Hash::from(Sha256::digest(cmd_bytes))
    );
    let null_bytes: Vec<u8> = vec![0x00, 0x02, 4, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0];
    let null = Entry::Null { client: 4, sn: 5 };
    assert_eq!(literal_leaf_bytes(&null), null_bytes);
    assert_eq!(leaf_hash(&null), Hash::from(Sha256::digest(null_bytes)));
    let nop_bytes: Vec<u8> = vec![0x00, 0x03, 9, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        leaf_hash(&Entry::Nop(9)),
        Hash::from(Sha256::digest(nop_bytes))
    );
}

#[test]
fn forest_roots_are_the_complete_tree_roots_of_the_binary_decomposition_of_the_sequence() {
    // §5: the forest holds a complete binary Merkle tree of 2^{i_b} leaves for each set bit of
    // m, tallest first, over consecutive runs of the sequence. Oracle: plain recursion over
    // the literal leaf bytes, for sequences mixing commands, nulls and no-ops.
    let sequence: Vec<Entry> = (0..40u64)
        .map(|i| match i % 3 {
            0 => Entry::Nop(i),
            1 => cmd((i % 5) as u32, i, 1000 + i),
            _ => Entry::Null {
                client: (i % 4) as u32,
                sn: i,
            },
        })
        .collect();
    let mut forest = MmrForest::new();
    for (idx, entry) in sequence.iter().enumerate() {
        forest.append(leaf_hash(entry));
        let m = idx + 1;
        let mut expected = Vec::new();
        let mut start = 0usize;
        for height in binary_decomposition(m as u64) {
            let size = 1usize << height;
            expected.push(complete_tree_root(&sequence[start..start + size]));
            start += size;
        }
        assert_eq!(forest.roots(), expected, "m = {m}");
    }
}

#[test]
fn forest_shape_is_the_binary_decomposition_of_m_for_every_m() {
    let mut forest = MmrForest::new();
    for m in 1..=64u64 {
        forest.append(leaf_hash(&Entry::Nop(m)));
        assert_eq!(forest.len(), m);
        assert_eq!(forest.peak_heights(), binary_decomposition(m), "m = {m}");
        assert_eq!(forest.roots().len(), m.count_ones() as usize, "m = {m}");
    }
}

#[test]
fn an_early_chain_still_verifies_after_the_tree_grows_past_it() {
    // A client whose newest ack is sn 2 holds a frozen chain for sn 1 (no
    // later chains to stitch). After foreign commits merge the forest well
    // past that pair, sn 1 still verifies: it stays in the window's previous
    // slot, so the stored record confirms it (the trivial position match).
    let mut server = ServerCertState::new();
    let mut client = ClientChains::new(7);
    for sn in 1..=2u64 {
        let e = cmd(7, sn, 700 + sn);
        client.record_issue(&e);
        server.append(&e);
        client.on_commit_ack(sn, server.notification(7));
    }
    for i in 0..30u64 {
        server.append(&Entry::Nop(2000 + i));
    }
    let cert = client.build_certificate(1).unwrap();
    assert!(server.verify(7, &cert), "bridge via stored newest chain");
}

#[test]
fn verification_bridges_via_the_previous_slot_when_the_server_runs_one_ahead() {
    // Wait-policy timing: sn 3 is committed server-side but its ack is still
    // in flight, so the window is (sn 2, sn 3) while the client's newest
    // chained command x_{k+1} is sn 2 — stored in the PREV slot. p. 29 only
    // needs hc(x_{k+1}) "still stored"; the bridge must work from either slot.
    // Geometry that defeats a last-slot-only bridge: sn 1 at pos 1 (chain
    // frozen at length 1 when sn 2 was acked at m = 3), sn 3 across the
    // power-of-8 boundary at pos 8 → lca-child level vs sn 3 is 3 > 1, but
    // vs sn 2 (pos 2, the prev slot) it is 1.
    let mut server = ServerCertState::new();
    let mut client = ClientChains::new(7);
    server.append(&Entry::Nop(0));
    let issue = |client: &mut ClientChains, server: &mut ServerCertState, sn: u64| {
        let e = cmd(7, sn, 700 + sn);
        client.record_issue(&e);
        server.append(&e);
    };
    issue(&mut client, &mut server, 1); // pos 1
    client.on_commit_ack(1, server.notification(7));
    issue(&mut client, &mut server, 2); // pos 2
    client.on_commit_ack(2, server.notification(7));
    for i in 0..5u64 {
        server.append(&Entry::Nop(2000 + i)); // pos 3..=7
    }
    issue(&mut client, &mut server, 3); // pos 8, ack in flight
    for i in 0..8u64 {
        server.append(&Entry::Nop(3000 + i));
    }
    let cert = client.build_certificate(1).unwrap();
    assert!(
        server.verify(7, &cert),
        "bridge via prev-slot hc(x_{{k+1}})"
    );
}

#[test]
fn duplicate_position_notifications_cannot_break_certificate_building() {
    // Notifications are external input: two carrying the same prev_pos for
    // different sns must not panic (debug underflow in lca_child_level) or
    // silently corrupt the stitched chain — the duplicate is skipped.
    let (server, mut client) = committed_run(4);
    client.on_commit_ack(
        4,
        Some(Notification {
            prev_chain: vec![[7u8; 32]; 2],
            prev_pos: client.chain_for(1).unwrap().0,
            prev_sn: 9,
        }),
    );
    client.record_issue(&cmd(7, 9, 709));
    let cert = client.build_certificate(1).unwrap();
    assert!(
        server.verify(7, &cert),
        "sn 1 unaffected by the poisoned entry"
    );
}

#[test]
fn verify_binds_the_certificate_to_the_claimed_client() {
    // Strengthening beyond the paper (whose root-comparison case is client-
    // agnostic): the entry's own client id must match the claimed client, so
    // per-client verification tallies cannot be cross-contaminated.
    let (server, client) = committed_run(4);
    let cert = client.build_certificate(2).unwrap();
    assert!(server.verify(7, &cert));
    assert!(
        !server.verify(99, &cert),
        "foreign client id must not verify"
    );
}

#[test]
fn forged_certificate_for_an_uncommitted_command_is_rejected() {
    let (server, _) = committed_run(6);
    let never_committed = cmd(7, 3, 999_999);
    for pos in 0..12u64 {
        for len in 0..5usize {
            let forged = protocol::certificates::Certificate::Chained {
                entry: never_committed.clone(),
                pos,
                chain: vec![[0u8; 32]; len],
            };
            assert!(!server.verify(7, &forged), "pos {pos} len {len}");
        }
    }
}

#[test]
fn accepted_certificates_may_carry_garbage_beyond_the_used_prefix() {
    // Deliberate prefix-fold semantics (Lemma 5.1 / p. 29): verification
    // consults only the prefix up to the confirmable node — acceptance does
    // NOT certify trailing links. Pinned so a refactor back to whole-chain
    // folding (or a downstream "accepted ⇒ all links correct" assumption)
    // fails loudly here.
    let (server, client) = committed_run(6);
    let protocol::certificates::Certificate::Chained {
        entry,
        pos,
        mut chain,
    } = client.build_certificate(2).unwrap()
    else {
        panic!()
    };
    chain.push([0xab; 32]);
    chain.push([0xcd; 32]);
    let padded = protocol::certificates::Certificate::Chained { entry, pos, chain };
    assert!(server.verify(7, &padded));
}

#[test]
fn forgotten_commands_and_their_order_remain_provable_from_hashes_alone() {
    // §5's headline property: the compact rule forgets committed commands,
    // yet certificates prove WHAT committed and WHERE in the order, from
    // O(log m) root hashes + two chains per client — nothing else.
    let (server, client) = committed_run(8);

    // Forgotten: sn 1..=6 evicted from every window slot of client 7…
    let (prev, last) = server.last_two(7).unwrap();
    for sc in prev.iter().copied().chain([last]) {
        assert!(sc.sn >= 7, "window holds only the last two (sn {})", sc.sn);
    }
    // …and the server-side certificate state is only the forest peaks
    // (≤ log₂ m + 1 roots) plus those two chains — no command bytes.
    let m = server.forest().len();
    assert_eq!(server.forest().roots().len() as u32, m.count_ones());

    // Every forgotten command still proves, and the verified positions
    // reproduce the exact commit order.
    let mut positions = Vec::new();
    for sn in 1..=6u64 {
        let cert = client.build_certificate(sn).unwrap();
        assert!(server.verify(7, &cert), "forgotten sn {sn} proves");
        let protocol::certificates::Certificate::Chained { pos, .. } = &cert else {
            panic!("evicted commands are chained certs")
        };
        positions.push(*pos);
    }
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "commit order: {positions:?}"
    );

    // Order is cryptographically BOUND, not asserted: swapping two
    // certificates' positions makes both reject.
    let a = client.build_certificate(2).unwrap();
    let b = client.build_certificate(4).unwrap();
    let (
        protocol::certificates::Certificate::Chained {
            entry: ea,
            pos: pa,
            chain: ca,
        },
        protocol::certificates::Certificate::Chained {
            entry: eb,
            pos: pb,
            chain: cb,
        },
    ) = (a, b)
    else {
        panic!()
    };
    let a_at_b = protocol::certificates::Certificate::Chained {
        entry: ea,
        pos: pb,
        chain: ca,
    };
    let b_at_a = protocol::certificates::Certificate::Chained {
        entry: eb,
        pos: pa,
        chain: cb,
    };
    assert!(
        !server.verify(7, &a_at_b),
        "sn 2 at sn 4's position rejects"
    );
    assert!(
        !server.verify(7, &b_at_a),
        "sn 4 at sn 2's position rejects"
    );
}

fn forest_of(ops: &[u64]) -> MmrForest {
    let mut forest = MmrForest::new();
    for &op in ops {
        forest.append(leaf_hash(&Entry::Nop(op)));
    }
    forest
}

#[test]
fn forests_of_different_length_are_compared_on_their_shared_leading_peaks() {
    let ops: Vec<u64> = (0..7).collect();
    let m6 = forest_of(&ops[..6]);
    let m7 = forest_of(&ops);
    assert!(m6.consistent_with(&m7) && m7.consistent_with(&m6), "prefix");

    // Heights [2, 1] against [2, 1, 0]: the height-2 peak covers leaves 0..4 in both.
    let mut forked = ops[..6].to_vec();
    forked[1] = 99;
    assert!(
        !forest_of(&forked).consistent_with(&m7),
        "fork inside leaf 0..4"
    );

    // Equal length: all peaks are comparable.
    let mut last = ops[..6].to_vec();
    last[5] = 99;
    assert!(
        !forest_of(&last).consistent_with(&m6),
        "equal m, last leaf differs"
    );

    // Heights [1, 0] against [2]: no shared leading peak, so nothing to compare.
    let mut other = ops[..4].to_vec();
    other[3] = 99;
    assert!(forest_of(&ops[..3]).consistent_with(&forest_of(&other)));
    assert!(MmrForest::new().consistent_with(&m7));
}

#[test]
fn a_newest_certificate_is_bound_to_the_command_content_in_either_window_slot() {
    let (server, _) = committed_run(4);
    for sn in [3u64, 4] {
        let honest = Certificate::Newest {
            entry: cmd(7, sn, 700 + sn),
        };
        assert!(server.verify(7, &honest), "sn {sn} honest");
        let forged = Certificate::Newest {
            entry: cmd(7, sn, 999_999),
        };
        assert!(!server.verify(7, &forged), "sn {sn} right sn, wrong op");
    }
    let wrong_sn = Certificate::Newest {
        entry: cmd(7, 5, 704),
    };
    assert!(!server.verify(7, &wrong_sn), "right op, uncommitted sn");
}

#[test]
fn case_b_decides_when_the_chain_stops_short_of_the_covering_peak() {
    // x1 at 0, x2 at 2, x3 at 5; the client acked x3 (so it stitched x1's chain to length 2) and
    // x4 is not committed. After the forest grows to one height-3 tree, x1's chain is too short
    // for case (a), x1 has left the window, and only the stored hc(x_{k+1}) confirms it.
    let mut server = ServerCertState::new();
    let mut client = ClientChains::new(7);
    let mut next_nop = 0u64;
    let mut place =
        |server: &mut ServerCertState, client: &mut ClientChains, sn: Option<u64>| match sn {
            Some(sn) => {
                let e = cmd(7, sn, 700 + sn);
                client.record_issue(&e);
                server.append(&e);
                client.on_commit_ack(sn, server.notification(7));
            }
            None => {
                server.append(&Entry::Nop(next_nop));
                next_nop += 1;
            }
        };
    for sn in [Some(1), None, Some(2), None, None, Some(3), None, None] {
        place(&mut server, &mut client, sn);
    }
    assert_eq!(server.forest().peak_heights(), vec![3]);
    let (prev, last) = server.last_two(7).unwrap();
    assert_eq!(
        (prev.unwrap().sn, last.sn),
        (2, 3),
        "x1 is out of the window"
    );

    let cert = client.build_certificate(1).unwrap();
    let Certificate::Chained { chain, .. } = &cert else {
        panic!("sn 1 is chained")
    };
    assert_eq!(chain.len(), 2, "shorter than the height-3 covering peak");
    assert!(server.verify(7, &cert));
}

#[test]
fn a_lost_chain_can_stall_the_extension_of_an_earlier_certificate() {
    // The client extends hc(x_j) only from the chains of later commands it received (§5 p. 28).
    // Client 7 at 0, 2, 4, 8, 10; the chain for sn 2 is lost, so sn 1 stays at length 1, short of
    // the height-3 peak and of the level-3 bridge to the window (x4, x5).
    let mut server = ServerCertState::new();
    let mut intact = ClientChains::new(7);
    let mut lossy = ClientChains::new(7);
    let mut sn = 0u64;
    for pos in 0..11u64 {
        if [0, 2, 4, 8, 10].contains(&pos) {
            sn += 1;
            let e = cmd(7, sn, 700 + sn);
            intact.record_issue(&e);
            lossy.record_issue(&e);
            server.append(&e);
            let note = server.notification(7);
            intact.on_commit_ack(sn, note.clone());
            lossy.on_commit_ack(sn, note.filter(|n| n.prev_sn != 2));
        } else {
            server.append(&Entry::Nop(pos));
        }
    }
    assert!(server.verify(7, &intact.build_certificate(1).unwrap()));
    let stalled = lossy.build_certificate(1).unwrap();
    assert!(matches!(&stalled, Certificate::Chained { chain, .. } if chain.len() == 1));
    assert!(
        !server.verify(7, &stalled),
        "sn 1 forfeits with sn 2's chain"
    );
    assert!(
        server.verify(7, &lossy.build_certificate(3).unwrap()),
        "sn 3 still verifies"
    );
}

#[test]
fn no_mutated_certificate_verifies_unless_it_names_the_committed_entry_at_its_position() {
    // Soundness oracle independent of the verifier: the committed sequence itself.
    let mut server = ServerCertState::new();
    let mut client = ClientChains::new(7);
    let mut ledger: Vec<Entry> = Vec::new();
    let mut sn = 0u64;
    for i in 0..40u64 {
        let entry = if i % 3 == 0 {
            sn += 1;
            let e = cmd(7, sn, 700 + sn);
            client.record_issue(&e);
            e
        } else {
            Entry::Nop(i)
        };
        server.append(&entry);
        ledger.push(entry);
        if sn > 0 {
            client.on_commit_ack(sn, server.notification(7));
        }
    }
    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move |bound: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % bound
    };
    let m = ledger.len() as u64;
    let mut accepted_mutants = 0;
    for want in 1..=sn {
        let Certificate::Chained { entry, pos, chain } = client.build_certificate(want).unwrap()
        else {
            continue;
        };
        for _ in 0..400 {
            let (mut e, mut p, mut c) = (entry.clone(), pos, chain.clone());
            match next(5) {
                0 => p = next(m + 3),
                1 => e = cmd(7, 1 + next(sn + 2), 700 + next(sn + 2)),
                2 => c.truncate(next(c.len() as u64 + 1) as usize),
                3 => c.push([next(256) as u8; 32]),
                _ => {
                    let i = next(c.len() as u64) as usize;
                    c[i][next(32) as usize] ^= 1 << next(8);
                }
            }
            let cert = Certificate::Chained {
                entry: e.clone(),
                pos: p,
                chain: c,
            };
            if server.verify(7, &cert) {
                assert_eq!(ledger.get(p as usize), Some(&e), "accepted a forgery");
                accepted_mutants += 1;
            }
        }
    }
    assert!(
        accepted_mutants > 0,
        "no mutant verified, so the oracle never compared anything"
    );
}
