# protocol

The rules from Cachin, Dou, Scheideler, Schneider, "A Lightweight Approach for State
Machine Replication" (arXiv:2509.17771v2), as pure per-node state machines. Page numbers
below refer to that version.

A node here never sends or receives anything. The driver (`crates/sim` or
`crates/network-node`) delivers requests and replies, draws which servers to contact,
and calls the node's step function once per round. Box steps that only send messages
are therefore driver steps and have no code in this crate.

Layout:

- `src/alg/` – one file per algorithm box.
- `src/support/` – data structures the rules run on (chunked log, merge, shared
  state). No protocol decision is made there.
- `src/certificates.rs` – §5 certificates (Merkle forest, client chains, Theorem 6
  verification).

## Box to code

Read each file next to its box in the PDF. Every bullet of a box is marked in the source by
a one-line comment that quotes the bullet's opening words, for example
`// Alg 5: "if i receives at least ℓ replies"`, placed where the bullet's code starts. The
anchors of one box appear in the file in the same order as the bullets in the box. The
table lists every anchor; `step_tags` (a unit test in `src/tag_map.rs`) checks that the
anchors in `src/` and this table agree, including their order.

The same names recur in every file: the paper's variables are fields (`x_i`, `l_i`, `s_i`,
`r_i`, `c_i`; a peer's are `l_j`, `s_j`, `c_j`, `r_j`), `None` is ⊥, the reply bullet is
`answer`, and the two reply-count bullets are `at_least_ell_replies` /
`fewer_than_ell_replies` where they are more than a few lines. Bullets that only send
messages run in the driver and are listed as "driver".

| Anchor | File | Function |
|---|---|---|
| driver | | Alg 1 (p. 10): "send k value requests to servers chosen uniformly and independently at random" |
| driver | | Alg 2 (p. 14): "send k value requests to servers chosen uniformly and independently at random" |
| `Alg 1: "if x_i ≠ ⊥ then for any value request received from some server j"` | `alg/median.rs` | `MedianNode::answer` |
| `Alg 2: "if x_i ≠ ⊥ then for any value request received from some server j"` | `alg/median.rs` | `MedianNode::answer` |
| `Alg 1: "if at least ℓ replies are received"` | `alg/median.rs` | `MedianNode::step` |
| `Alg 1: "if less than ℓ replies are received"` | `alg/median.rs` | `MedianNode::step` |
| `Alg 2: "if at least ℓ replies are received"` | `alg/median.rs` | `MedianNode::step_with` |
| `Alg 2: "if less than ℓ replies are received"` | `alg/median.rs` | `MedianNode::step_with` |
| `Alg 2: "set x_i := f(S)", with f = largest (Definition 3.5)` | `alg/priority.rs` | `PriorityNode::step` |
| driver | | Alg 3 (p. 20): "server i sends k log requests to servers chosen uniformly and independently at random" |
| `Alg 3: "for every command x received from a client that is not yet contained in L_i"` | `alg/extended.rs` | `LogNode::wants_amplify` |
| `Alg 3: "if L_i ≠ ⊥ then for any log request received by server i from a server j"` | `alg/extended.rs` | `LogNode::answer` |
| `Alg 3: "if server i receives at least ℓ replies"` | `alg/extended.rs` | `LogNode::step` |
| `Alg 3: "server i chooses a subset M of size ℓ from the received logs"` | `alg/extended.rs` | `LogNode::step` |
| `Alg 3: "server i sets L_i := L'_i ∘ L̄"` | `alg/extended.rs` | `LogNode::step` |
| `Alg 3: "if server i receives less than ℓ logs"` | `alg/extended.rs` | `LogNode::step` |
| driver | | Alg 4 (p. 21): "send k value requests to servers chosen uniformly and independently at random" |
| `Alg 4: "if x_i ≠ ⊥ then for any value request received from some server j"` | `alg/gossip.rs` | `GossipNode::answer` |
| `Alg 4: "if at least one reply was received that contains x"` | `alg/gossip.rs` | `GossipNode::step` |
| `Alg 4: "if less than ℓ values are received"` | `alg/gossip.rs` | `GossipNode::step` |
| driver | | Alg 5 (p. 26): the σ log n append requests of `Triage::Amplify` and the k requests themselves |
| `Alg 5: "for every command x received from a client c"` | `alg/compact.rs` | `CompactNode::on_client_command` |
| `Alg 5: "if L_i ≠ ⊥, sn(x) = sn(c) + 1 and x ∉ L_i"` | `alg/compact.rs` | `CompactNode::on_client_command` |
| `Alg 5: "if L_i ≠ ⊥ and sn(x) = sn(c)"` | `alg/compact.rs` | `CompactNode::on_client_command` |
| `Alg 5: "otherwise, i ignores x"` | `alg/compact.rs` | `CompactNode::on_client_command` |
| `Alg 5: "i sends k requests ... and attaches a bit b_i"` | `alg/compact.rs` | `CompactNode::b_i` |
| `Alg 5: "if L_i ≠ ⊥ then for any request received by i from some server j"` | `alg/compact.rs` | `CompactNode::answer` |
| `Alg 5: "if i receives at least ℓ replies"` | `alg/compact.rs` | `CompactNode::at_least_ell_replies` |
| `Alg 5: "i chooses a subset M of ℓ of the replies uniformly at random"` | `alg/compact.rs` | `CompactNode::at_least_ell_replies` |
| `Alg 5: "i sets L_i := L'_i ∘ L̄"` | `alg/compact.rs` | `CompactNode::at_least_ell_replies` |
| `Alg 5: "if the logs in L_i contain different commands from the same client with the same sequence number"` | `alg/compact.rs` | `CompactNode::at_least_ell_replies` |
| `Alg 5: "if b_i = 1 then i picks any reply S_j"` | `alg/compact.rs` | `CompactNode::at_least_ell_replies` |
| `Alg 5: "i determines the largest prefix P_i of L_i"` | `alg/compact.rs` | `CompactNode::at_least_ell_replies` |
| `Alg 5: "i commits the commands in P_i in the given order on S_i, removes P_i from L_i"` | `alg/compact.rs` | `CompactNode::at_least_ell_replies` |
| `Alg 5: "if i receives less than ℓ replies"` | `alg/compact.rs` | `CompactNode::fewer_than_ell_replies` |
| `Alg 5: "i sets L_i := ⊥"` | `alg/compact.rs` | `CompactNode::fewer_than_ell_replies` |
| `Alg 5: "if at least one reply is received then i picks any one of them"` | `alg/compact.rs` | `CompactNode::fewer_than_ell_replies` |
| driver | | Alg 6 (p. 31): "Send k requests to random servers" |
| `Alg 3: "for every command x received from a client that is not yet contained in L_i"` | `alg/recovery.rs` | `RecoveryNode::wants_amplify` |
| `Alg 5: "for every command x received from a client c"` | `alg/recovery.rs` | `RecoveryNode::on_client_command` |
| `Alg 5: "if L_i ≠ ⊥, sn(x) = sn(c) + 1 and x ∉ L_i"` | `alg/recovery.rs` | `RecoveryNode::on_client_command` |
| `Alg 5: "if L_i ≠ ⊥ and sn(x) = sn(c)"` | `alg/recovery.rs` | `RecoveryNode::on_client_command` |
| `Alg 5: "otherwise, i ignores x"` | `alg/recovery.rs` | `RecoveryNode::on_client_command` |
| `Alg 6: "If a request was received from server j and R_i ≠ ⊥"` | `alg/recovery.rs` | `RecoveryNode::answer` |
| `Alg 3: "if L_i ≠ ⊥ then for any log request received by server i from a server j"` | `alg/recovery.rs` | `RecoveryNode::answer` |
| `Alg 3: "if server i receives at least ℓ replies"` | `alg/recovery.rs` | `RecoveryNode::extended_median_rule` |
| `Alg 3: "server i chooses a subset M of size ℓ from the received logs"` | `alg/recovery.rs` | `RecoveryNode::extended_median_rule` |
| `Alg 3: "server i sets L_i := L'_i ∘ L̄"` | `alg/recovery.rs` | `RecoveryNode::extended_median_rule` |
| `Alg 3: "if server i receives less than ℓ logs"` | `alg/recovery.rs` | `RecoveryNode::extended_median_rule` |
| `Alg 6: "If at least ℓ replies were received"` | `alg/recovery.rs` | `RecoveryNode::at_least_ell_replies` |
| `Alg 6: "If no-reset was received in one reply"` | `alg/recovery.rs` | `RecoveryNode::at_least_ell_replies` |
| `Alg 6: "let C' = (S', P', W') be the received checkpoint with largest W'"` | `alg/recovery.rs` | `RecoveryNode::at_least_ell_replies` |
| `Alg 6: "If W' > W"` | `alg/recovery.rs` | `RecoveryNode::at_least_ell_replies` |
| `Alg 6: "If less than ℓ replies were received"` | `alg/recovery.rs` | `RecoveryNode::fewer_than_ell_replies` |
| `Alg 6: "If R_i = reset"` | `alg/recovery.rs` | `RecoveryNode::between_windows` |
| `Alg 6: "If L_i ≠ ⊥"` | `alg/recovery.rs` | `RecoveryNode::between_windows` |
| `Alg 6: "for current checkpoint C_i = (S, P, W), commit all commands in P on S_i and remove the prefix P from L_i"` | `alg/recovery.rs` | `RecoveryNode::between_windows` |
| `Alg 6: "make new checkpoint C_i := (S_i, P_i, W')"` | `alg/recovery.rs` | `RecoveryNode::between_windows` |
| `Alg 6: "set R_i := no-reset"` | `alg/recovery.rs` | `RecoveryNode::between_windows` |
| `Alg 6: "If L_i = ⊥"` | `alg/recovery.rs` | `RecoveryNode::between_windows` |

Algorithm 6's "choose C' with the largest W'" is the function `latest_checkpoint_index`.
`end_window` and `end_window_shared` both run the between-windows bullets through the one
function `between_windows`. `end_window_shared` differs only in "make new checkpoint": instead of building
a new checkpoint it holds an equal one that another node already built. It only saves memory and
adds no box step. `share_checkpoint` does the same for a checkpoint the node already holds. The
caller proves the equality (the simulator keys by a 64-bit fork signature), and the node checks
what the proof claims:

- `share_checkpoint` compares the peer's checkpoint with the node's C_i and asserts, in every
  build, equal W, equal P, equal committed length of S,
  equal sn maps, the same §5 gate (both or neither carry certificates) and the same §5 forest
  (leaf count and roots); in debug builds also equal retained executed entries.
- `end_window_shared` compares the peer's checkpoint with the one the node would mint and
  asserts, in every build, that the peer's W is W', the same §5 gate, that
  the peer's P equals P_i, equal committed length of S, equal sn maps, and that the node is not ⊥
  afterwards; in debug builds also equal retained executed entries and the same §5 forest.

Release builds therefore do not see executed entries that a digest collision would swap beneath
an equal length, and no build sees entries below a forget line. The operations on P (L_i := P,
execute P, remove P, P_i) are named functions in `support/checkpoint.rs`.

§5 has no algorithm box, so `certificates.rs` has no step tags. Its functions name the
paper objects directly (see the glossary).

## Choices the paper does not fix

Three kinds, each tagged in the source with its id:

- `// DETERMINIZED: [DET-n]` – the box leaves a choice open; the code fixes one.
- `// EXTENSION: [EXT-n]` – the code does more than the box, as the paper's prose allows.
- `// DEVIATION: [DEV-n]` – the code differs from a paper definition on purpose.

"KB" below means `.claude/knowledge/`.

| Id | Kind | Choice | Why | Where | Source |
|---|---|---|---|---|---|
| DET-1 | DETERMINIZED | The median comparator compares entries together with their round stamp. The paper orders command sequences and does not say whether the stamp counts. | Default for both rules. The paper admits (command, round stamp) as the key (Alg 5 box, p. 26; A5.OPEN.5). The one stored comparison of the two keys was made under a boundary prefix test that compared stamps; that test compares commands now (DET-15), so a stamp difference alone does not abort a boundary. The default stays because stored configs and numbers use it. `StampTie::CommandsOnly` is kept as an option. | `alg/compact.rs`, `alg/recovery.rs` | KB `protocol/algorithm-box-ambiguities.md`; `experiments/e9-recovery/prereg-amd8-stamp-tie.md` |
| DET-2 | DETERMINIZED | A command seen in two non-median logs keeps the stamp of the first log in sorted order. The box lets L̄ be built "in any order". | The first sighting is kept, later copies are dropped. `UnionStamp::LastSighting` is kept as an option. | `alg/compact.rs`, `alg/recovery.rs` | KB `protocol/algorithm-box-ambiguities.md` |
| DET-3 | DETERMINIZED | Alg 5 "if b_i = 1 then i picks any reply S_j": "any S_j in M" is the first reply in draw order that carries a state. | Any choice is allowed; draw order is fixed per seed. | `alg/compact.rs` | KB `paper/compact-median-rule.md` |
| DET-4 | DETERMINIZED | Alg 5 "if at least one reply is received then i picks any one of them": "any one" reply is the first reply received. That one reply gives both the state and the aged prefix (executed as in DET-13). | One reply is the unit, so state and prefix come from the same server. | `alg/compact.rs` | KB `paper/compact-median-rule.md` |
| DET-5 | DETERMINIZED | Under Alg 6 there are two ℓ-counts: Alg 6's "If at least ℓ replies" / "If less than ℓ replies" count every reply, Alg 3's merge counts only replies that carry a log. | Lemma 6.2 (R_i = ⊥ ⇒ L_i = ⊥) is one-way. A node in reset with L_i = ⊥ still answers and must be counted as a reset voter. | `alg/recovery.rs` | KB `paper/recovery-protocol.md` |
| DET-6 | DETERMINIZED | Among received checkpoints with equal largest W', the last reply wins. The box says "break ties arbitrarily". | `max_by_key` keeps the last maximum; `latest_checkpoint_index` names this. | `alg/recovery.rs` | KB `paper/recovery-protocol.md` |
| DET-7 | DETERMINIZED | In a round, the Alg 3 merge runs first. Checkpoint adoption (Alg 6 "If W' > W") runs after it and overwrites L_i with P'. The box says "on top of extended median rule" and gives no order. | The paper says a server with an older checkpoint "immediately adopts" the newer one (p. 30). | `alg/recovery.rs` | Alg 6 box (p. 31), p. 30; test `adoption_overrides_same_round_median_and_appends` in `tests/recovery_rule.rs` |
| DET-8 | DETERMINIZED | The §5 hash is SHA-256 with RFC 6962 leaf/interior prefixes. The paper asks only for a collision-resistant hash (p. 27). | A standard, domain-separated instantiation. | `certificates.rs` | KB `protocol/certificates-implementation.md` |
| DET-9 | DETERMINIZED | Committing a command sets sn(c) := sn(x); committing ⊥ for (c, sn) sets sn(c) := sn. The Alg 5 box says the sequence numbers are "incremented". | The two agree whenever a client's commands commit in sequence-number order, which the client bullet enforces (only sn(x) = sn(c) + 1 is amplified). Setting keeps the executed state a function of the committed entries. | `support/shared_state.rs` | Alg 5 box (p. 26) |
| DET-10 | DETERMINIZED | Entries are ordered Cmd < Null < Nop. Commands compare by (client, sn, op); no-ops by their number. Logs compare entry by entry, then by stamp (DET-1). | The median needs a total order. The paper names none for (client, sn, payload) commands and does not place ⊥, x₀ or x_d in it. The derived `Ord` of `Entry` and `Timed` gives this one. | `alg/compact.rs` | Alg 3 box (p. 20), Alg 5 box (p. 26) |
| DET-11 | DETERMINIZED | x_d is `Nop(round)` stamped with the current round, so it ages like a command and sorts above commands. The seed command x₀ is `Nop(0)` at round 0. | The box says only to append a dummy when a strip empties L_i. It gives no identity, stamp, position or commit rule. A no-op changes no state. | `alg/compact.rs` | Alg 5 box (p. 26), Alg 6 box (p. 31) |
| DET-12 | DETERMINIZED | L̄ is built from the sorted logs of M in sorted order (the median first), then from the append requests in the order the driver passes them. Both drivers pass them by sender id. The box lets L̄ be built "in any order". | One fixed order keeps a run a function of its seed. Alg 3 (`extended.rs`) uses the same walk over bare commands. | `alg/compact.rs`, `alg/recovery.rs`, `alg/extended.rs` | Alg 3 box (p. 20), Alg 5 box (p. 26) |
| DET-13 | DETERMINIZED | With at least one but fewer than ℓ replies, Alg 5 "performs the updates ... as described above": the first reply's aged prefix is executed on S_i as it stands, after adopting S_j when b_i = 1. No other filter applies. | When b_i = 0 the reply carries no S_j, so executing the prefix is the only update left. The box has no commit-time validity check (the same holds for the ℓ-reply path). | `alg/compact.rs` | Alg 5 box (p. 26) |
| DET-14 | DETERMINIZED | A rollback to the genesis checkpoint (S, P = ⊥, W = 0) sets L_i := ⊥, not an empty valid log. Adopting or rolling back to an empty prefix (P = the empty sequence) gives an empty log that is not ⊥. | Alg 6 keeps ⊥ and the empty prefix apart, and "L_i := P" is read literally for both. See the note below on Theorem 7. | `support/checkpoint.rs` | Alg 6 box (p. 31) |
| DET-15 | DETERMINIZED | The boundary prefix test compares commands, not stamps. "Remove the prefix P from L_i" drops L_i's first len(P) entries; the kept suffix keeps L_i's own stamps. P's stamps never reach S_i. | Alg 6 defines P as a "sequence of pre-committed commands"; stamps are age bookkeeping. The box does not say what happens to the stamps of P and of the suffix. | `support/checkpoint.rs` | Alg 6 box (p. 31) |
| DET-16 | DETERMINIZED | Alg 6 "If no-reset was received in one reply" sets R_i := no-reset if any of the received replies carries no-reset, and the checkpoint rule adopts the largest W' over all replies iff W' > W. Def 3.5, which §6 cites for both updates (pp. 32–34), would take the extreme of an ℓ-subsample that excludes the own value. | The code follows the box and the paraphrases of Lemma 6.4, which scan all replies; the box is normative. `PriorityNode` implements the literal Def 3.5 reading, so the two rules differ. | `alg/recovery.rs` | KB `paper/recovery-protocol.md` |
| DET-17 | DETERMINIZED | A server with L_i = ⊥ ignores client commands (Alg 5's intake guard). Alg 3, the base of Alg 6, has no such guard. | The subsumed compact rule (§6, p. 30) carries the guard. `RecoveryNode::wants_amplify` follows Alg 3 literally (⊥ holds no command) but is only asked about commands a non-⊥ server accepted. | `alg/recovery.rs` | Alg 5 box (p. 26) |
| DET-18 | DETERMINIZED | A certificate does not name x_{k+1}. The verifier tries case (a) first (fold the first h entries to the root of the tree covering p(x), h being that tree's height), then case (b) against each of the client's two stored records in turn; the first d entries of a chain are used and the rest is ignored. | §5 says only that x_{k+1} is "one of the two last" commands (p. 29) and leaves open which root and how long a chain may be (C.OPEN.19, C.OPEN.21, C.OPEN.22 of the reading notes). | `certificates.rs` | §5 p. 29 |
| DET-19 | DETERMINIZED | The server's window of two records per client rotates on every committed entry that carries (client, sn): commands and ⊥. A no-op does not rotate it. The verifier then accepts a certificate for a stored ⊥ entry like one for a command. | §5 stores "the last two commands" (p. 27) and does not say what a committed ⊥ does to them (C.OPEN.10). | `certificates.rs` | §5 p. 27 |
| DET-20 | DETERMINIZED | The client extends hc(x_j) from the chains it received for later commands, and from nothing else. | §5 p. 28 names the extension (Lemma 5.1) but gives no algorithm (C.OPEN.17). | `certificates.rs` | §5 p. 28 |
| EXT-2 | EXTENSION | §5 certificate state is carried in Alg 6 checkpoints. P is attested when it is executed at the boundary. | §6 says the §5 techniques "can be adapted" (p. 30); P is final only at the boundary (Lemma 6.9). Without certificates (`certs = false`) Alg 6 runs as printed. | `alg/recovery.rs` | KB `findings/recovery/certificates-over-recovery-construction.md` |
| EXT-3 | EXTENSION | Opt-in (`RepeatedCommit::Skip`; the default `Execute` runs the boxes as printed). The replicated state machine is execute-once: a command whose sn(x) ≤ sn(c) leaves S unchanged (cf. Raft client sessions, Ongaro 2014 §6.3). The same holds for ⊥, so sn(c) never moves backwards. This is a property of the state machine, not a change to the protocol: logs, merges and checkpoints are untouched. Used by Alg 5's commit (both reply paths) and Alg 6's boundary commit. Each skipped entry is counted per node, since a skip means agreement already failed upstream. Precondition: a client's commits come in sn order. Alg 5 intake admits only sn(x) = sn(c) + 1, but once agreement has failed a server can still commit sn s + 1 before sn s (seen with pool clients under blocking, in compact and recovery runs). Without the precondition the rule drops a command that was never executed (sn 3 committed before sn 2 makes sn 2 look done). So under `Skip`, a P holding an sn(x) > sn(c) + 1 is refused whole: no entry of P is executed. The box steps before the commit stay done, as the box orders them: in Alg 5 an S_j adopted at b_i = 1 stays adopted, and L_i keeps its value from before the round (≥ ℓ replies) or stays ⊥ (< ℓ replies); in Alg 6 a rollback (S_i := C_i.S, L_i := P) stays done and no checkpoint is minted. The node records the refusal and is never stepped again, and the simulator ends the run as a recorded failure (phase `OutOfSnOrderCommit`, grid abort kind `sn_order`). Not combined with §5 certificates (the checkpoint would attest a skipped command again); construction refuses it. | §6 says the techniques of §§4–5, client sequence numbers among them, "can be adapted to work in the recovery protocol" and then assumes unique commands (p. 30); sn(c) is already part of S_i (Alg 5, p. 26). A committed command can re-enter a log after other servers stripped it (a surviving copy re-spreads through the median union) and would otherwise commit a second time. The rule reads only S_i and the entry, so servers with equal states stay equal. It does not reconcile states that already disagree. | `support/shared_state.rs`, `alg/compact.rs`, `alg/recovery.rs` | Alg 5 box (p. 26), §6 (p. 30); tests `skip_*` in `support/shared_state/tests.rs`, `alg/compact/tests.rs`, `tests/recovery_rule.rs` |
| DEV-2 | DEVIATION | Certificate verification also requires the entry to belong to the verifying client. The paper's root case (a) does not check this. | Keeps per-client verification counts from mixing. | `certificates.rs` | Theorem 6 proof (p. 29); test `verify_binds_the_certificate_to_the_claimed_client` in `tests/paper/certificates.rs` |
| DEV-3 | DEVIATION | Opt-in (`PrefixMismatch::SkipBoundary`; the default `Abort` panics as printed). At a boundary where R_i ≠ reset, L_i ≠ ⊥ and P is not a prefix of L_i, the node skips the between-window steps: nothing is executed, no checkpoint is minted, C_i keeps its old W, and L_i := ⊥ (so R_i := reset by the box's last step). The skipped C_i is marked stale: later boundaries skip the same way (no reset restore, no commit of the stale P) until "If W' > W" adopts a newer peer checkpoint. | Alg 6 (p. 31) has no branch for this case; Lemma 6.7 rules it out only w.h.p., and a near-tie order vote at the minting boundary produces it in practice. A skipping node never commits the losing P, and adoption pulls it onto the majority checkpoint. The stale C_i is still served to peers (R_i is reset, not ⊥), so a peer with an older W can adopt it with the mark cleared and commit that P. L_i must go to ⊥: peers strip what they commit at that boundary, and a kept unstripped log re-gossips those commands into every log for a second commit (seen on seed 4331438). | `alg/recovery.rs` | Unregistered prototype; `experiments/e9-recovery/log.md` (prefix-abort diagnosis) |
| DEV-4 | DEVIATION | The commit-ack payload is (hc(x_1), p(x_1), sn(x_1)). The client files the chain under the received sn(x_1). | §5 sends (hc(x_1), p(x_1)) (p. 28) and the client could take sn(x_1) as sn − 1. The two differ when the server's second-to-last record is not the command before the acked one. Filing under the received sn keeps a chain from being attached to the wrong command. The extra field is one u64. | `certificates.rs` | §5 p. 28 |

### Open points, alternatives, bias

Each entry names the open point the choice resolves, the other admissible readings, and
whether the choice can move a measurement. "A5.OPEN.n", "A3.OPEN.n", "A6.OPEN.n" and "C.OPEN.n"
number the open points of the per-algorithm reading notes for Alg 3, Alg 5, Alg 6 and §5
(`docs/paper-spec/`); the page is
where the box sits in the PDF.

- **DET-16, DET-6 – the R and C updates over all replies** (A6.OPEN.1; Alg 6, pp. 31–34; Def 3.5,
  p. 23). Other readings: apply Def 3.5 literally, that is, an ℓ-subsample with the own value
  excluded for both updates (what `PriorityNode` does for Alg 2); break W' ties by the first
  reply or at random. The box and its paraphrases scan every reply, so a lone no-reset among
  k = 6 replies always wins and the largest W' is always adopted; the Def 3.5 reading would adopt
  a lone no-reset only with probability about 1/2 and could pick a smaller W'. Equal-W' ties go
  to the last reply in slot order, which is uniform among tied replies under i.i.d. targets. Can
  bias: recovery follows the box law, so no-reset and newer checkpoints spread faster than the
  Cor 3.6 model behind Lemmas 6.1, 6.4 and 6.5; timings measured for `PriorityNode` do not
  calibrate Alg 6. Tests: `any_no_reset_reply_wins_over_all_received_replies_not_a_subsample`,
  `a_tie_on_the_largest_window_resolves_to_the_last_tied_reply`.
- **DET-17 – client commands at a ⊥ server** (A6.OPEN.26; Alg 5, p. 26; Alg 3, p. 20). Other
  reading: Alg 3 without a guard, so a ⊥ server also amplifies. A command that reaches a
  non-blocked ⊥ server is dropped and the client resends. Can bias: injection success during and
  right after a surge is lower than under the other reading; latency of commands injected then is
  longer.
- **DET-7 – merge before adoption** (A6.OPEN.2; Alg 6, p. 31; p. 30 "immediately adopts"). Other
  readings: adoption first and the Alg 3 merge overwrites P'; skip the merge in a round with
  adoption; adopt only if the merge left L_i = ⊥. The code merges, then adopts, so an adopting
  server drops the merged log and the append requests of that round; their clients resend. Can
  bias: a small latency effect on commands injected at servers that adopt in that round. Test:
  `adoption_overrides_same_round_median_and_appends`.
- **DET-1, DET-10 – median key and entry order** (A5.OPEN.5, A5.OPEN.6, A5.OPEN.7; Alg 5, p. 26;
  Alg 3, p. 20; recovery inherits it, p. 31). Other readings: commands only
  (`StampTie::CommandsOnly`); a different total order on K; ⊥ or no-ops placed first or last. The
  stamp of an early entry can decide which log is the median, and so the command order that
  follows. Can bias: possibly, through convergence time and the choice of median in contested
  rounds; no stored experiment measures it. The only stored comparison of the two keys (Amendment 8 of the recovery experiment) was made before the
  prefix test became command-level (DET-15). The tests `command_only_median_alternative_*` in
  `tests/paper_fidelity.rs` run a `StampTie::CommandsOnly` recipient and require the command
  median, and require the default key to differ on the same fixture: a sensitivity check, not a
  fidelity requirement.
- **DET-11 – the dummy x_d** (A5.OPEN.32; Alg 5, p. 26; Alg 6, p. 31). Other readings: a global
  constant; a never-committed sentinel; a lowest-order marker. Because x_d is stamped "now" and sorts
  above commands, a log `[x_d(r)]` beats `[a@s]` in the median. A command behind a young x_d cannot
  commit on that server before round r + T. Can bias: only when a strip empties a log; commit rounds of
  the commands behind it can then differ between servers. x₀ = `Nop(0)` at round 0 is already T rounds
  old at the first boundary, so it is the first P; the dummies are no-ops, so no state changes.
- **DET-12, DET-2 – order and stamps of L̄** (A3.OPEN.14, A5.OPEN.11 for the order; A5.OPEN.10,
  A5.OPEN.45 for the stamp; Alg 3, p. 20; Alg 5, p. 26). Other readings for the order: sorted by K,
  by (round, command), arrival order, random. Other readings for the stamp: oldest, youngest, median
  of the stamps, a logs-over-appends rule, or re-stamping on receipt. The code walks the sorted M
  logs in order, then the appends; a command keeps the stamp of its first sighting, so a log's stamp
  beats an append's stamp, and an append keeps its sender's stamp. The sim passes appends by
  injecting server id and then client order; the network node sorts them by (sender, client, sn).
  Can bias: yes, in principle. The order shapes the variety of prefixes during broadcast, and the
  stamp sets the commit round. The paper's bound for Alg 3 (Theorem 2) holds for any order, so
  only measured constants move. `UnionStamp::LastSighting` is the stamp sensitivity arm; it re-stamps only entries of L̄, because the median L'_i is taken whole, so no duplicated ⊥ in L'_i is ever re-stamped. The order has no switch.
- **DET-13, DET-4 – the below-ℓ branch** (A5.OPEN.33, A5.OPEN.29; Alg 5, p. 26). Other readings
  of "performs the updates ... as described above": adopt S_j only; execute the prefix only; both
  (the code); filter the prefix against sn(c). When b_i = 0 the reply holds no S_j, so only the prefix
  can be executed. L_i is already ⊥ at that point, so the node is not useful afterwards. Under the default
  `RepeatedCommit::Execute` a lagging reply can make this node execute a command twice. Can bias:
  only the committed sequence of nodes that lose their log; the effect shows once a double
  execution has happened.
- **DET-18 – which record is x_{k+1}, which root, how long a chain** (C.OPEN.19, C.OPEN.21,
  C.OPEN.22; §5 p. 29). Other readings: only the newest record (rejects an honest certificate
  while the client's next command is committed but not yet acknowledged); the smallest stored
  position above p(x); a client that names x_{k+1}; require exactly d entries; check that the
  tree at p(x) has height d. Case (a) folds to the covering tree's root, which is the paper's case A
  for every reference in a later tree. Can bias: the share of outdated chained certificates that
  still verify. E6 measures only outdated bare-newest certificates, which no reading accepts once
  the client has two further commits.
- **DET-19 – ⊥ and the window** (C.OPEN.10; §5 p. 27). Other readings: ⊥ and x_d do not touch the
  window. Under that reading the ack of a command after a ⊥ names the command before the ⊥ as x_1.
  Honest runs of the certificate experiments commit no ⊥. Can bias: only runs with committed ⊥.
- **DET-20 – client extension** (C.OPEN.17; §5 p. 28). Other readings: the client stores its own
  extended chains and extends them from every ack; or from every chain a server returns on request. With
  this reading, losing the chain of one command can leave an earlier command's chain short of the
  level the server needs, so §5's claim that a lost chain costs only its own command holds for some
  command layouts and not for others (`a_lost_chain_can_stall_the_extension_of_an_earlier_certificate`).
  Can bias: no experiment drops chains.
- **DEV-4 – the sn in the ack** (C.OPEN.13; §5 p. 28). The other reading is the paper's payload
  with sn(x_1) := sn − 1. Can bias: not in runs where each client's commands commit in sn order
  and no ⊥ commits, where the two values coincide.
- **DET-9 – sn(c) := sn(x)** (A5.OPEN.30, A5.OPEN.29; Alg 5, p. 26). Other readings: increment
  (the box); assign max(sn(c), sn(x)). They differ for stale commits, ⊥ commits and out-of-order
  commits. Under the default `Execute` a stale re-commit lowers sn(c) and the client bullet
  accepts that command again. Can bias: only after a first double execution; `RepeatedCommit::Skip`
  (EXT-3) removes it.
- **DET-14 – genesis rollback** (A6.OPEN.4; Alg 6, p. 31; Theorem 7, p. 30; Lemma 6.10, p. 37).
  Other readings: treat P = ⊥ as the empty sequence and the rolled-back log as an empty valid log,
  so the node appends x_d, mints a checkpoint and sets no-reset; or keep a non-empty P. The code
  reads "L_i := P" literally: every node holds (S, ⊥, 0) after a full death spiral in window 0, rolls
  back to L_i = ⊥, sets reset again at each boundary, and never revives. **This conflicts with
  Theorem 7's recovery claim under the other reading**: the paper claims recovery in O(log n) rounds
  after any surge, the code halts for good if every node reaches the genesis rollback with L_i = ⊥.
  Tests that pin the halt: `rollback_to_genesis_checkpoint_stays_reset` and
  `full_window_zero_surge_latches_reset_forever`. Can bias: yes, decisively. Whether a surge in
  window 0, or any surge with an empty newest P, is survived depends on this reading. The thesis states which
  reading it measures. An empty P (the empty sequence, which any boundary with no aged entry
  mints, for example under no client load) is not ⊥: adopting or rolling back to it leaves an
  empty log that answers log requests and enters medians. The simulator counts such a server as
  useful although §3 (p. 19) defines useful as holding a non-empty log. This is the other
  admissible reading of A6.OPEN.4; it revives a server mid-window and inflates the useful count
  slightly in quiet windows. Test: `adopted_empty_pre_leaves_empty_non_bot_log_without_mid_window_dummy`.
- **DET-15 – prefix test and stamps after the strip** (A6.OPEN.18; Alg 6, p. 31). Other readings
  for a P that is not a prefix: remove only the matching prefix; remove P's commands wherever they
  are; drop the first len(P) entries blindly (the code, once the commands match); flag a safety anomaly
  (the default `Abort`, and DEV-3 as the opt-in skip). The test compares commands, so a P whose
  commands lead L_i aborts nothing even if the stamps differ. The kept suffix keeps L_i's stamps and
  the next P_i is aged by them. Can bias: the stamp-only aborts are gone; a real command mismatch still
  aborts under the default, which censors that run (see below).
- **EXT-2 – the leaf set of the carried forest** (C.OPEN.5, C.OPEN.10; §5, pp. 27–28; Alg 6,
  p. 30). Other readings: every committed log entry is a leaf, including ⊥, x₀ and x_d, and a
  committed ⊥ rotates the client's last-two window. Recovery attests only the client commands
  that execute (`Entry::Cmd`), as does the compact harness of the simulator, so a leaf position is
  an index into the executed sequence. `ServerCertState::append` itself accepts any entry and
  hashes no-ops and ⊥ as leaves; the drivers never pass them. Can bias: nothing measured, since
  ⊥ commits are unreachable with unique clients; positions and the ack payload after a ⊥ would
  differ. Test: `only_committed_commands_enter_the_forest_not_nulls_or_nops`.
- **Equivocation ⊥** (A5.OPEN.24 c–f, A5.OPEN.46; Alg 5, p. 26; prose p. 25; no tag). Other readings:
  a ⊥ tagged with (c, sn) that stops a later copy of x being accepted or added to L̄; one ⊥ per
  conflicting pair instead of one per entry; skipping the rule under the no-equivocation
  assumption. The code replaces every conflicting command in place by a ⊥ that keeps its position and
  stamp. The client bullet tests `x ∉ L_i` by command, so a ⊥ for (c, sn) does not block x: x is
  accepted again, can re-enter through L̄, and then commits beside the ⊥ of its slot. Committing a
  ⊥ sets sn(c) and executes nothing. Tests: `a_bot_for_the_slot_does_not_block_the_command_and_both_commit`.
  Can bias: only under equivocating clients; no workload in the simulator, the lab or the node
  tier generates equivocation, so the stored experiments do not reach it.
- **Own log outside L̄** (A5.OPEN.13; Alg 3, p. 20; Alg 5, p. 26; Lemma 3.8 proof, p. 24; no tag).
  Other reading: server i's own previous log is a source of L̄ in addition to M and the appends. The
  code follows the box: the sources are the ℓ chosen replies (i's own log among them only if i drew
  itself and is not ⊥) and this round's appends, so a command that sits only in i's own log
  and in no reply of M or append is lost. Tests: `a_command_held_only_by_the_server_itself_is_dropped`
  in `tests/paper/extended_rule.rs` and `tests/paper/compact_rule.rs`. Can bias: command survival
  and broadcast speed for commands held by few servers.
- **S_i is the executed command list** (A5 state S_i is abstract, pp. 25–26; Theorem 5 bounds the
  space by the shared state; no tag). Other reading: a compact state (a digest or the §5 forest
  peaks) that does not grow with the history. `SharedState` keeps every executed command plus sn(c);
  protocol decisions read only sn(c). The simulator forgets a committed prefix only when its
  driver asks for it (lean and grid runs, following the observer's frontier); the node tier never
  forgets, so a state-carrying reply grows with the history and meets its frame cap in long runs.
  Can bias: per-server storage and per-transfer byte figures only, never a safety or liveness
  verdict.
- **§5 state beside the node** (C.OPEN.26; §5 p. 27, §6 p. 30; no tag). Other reading: the Merkle
  forest roots and the last two records per client are part of S_i, so they travel in S_j
  replies and are adopted with it. `CompactNode` holds no certificate fields; the driver's
  `CertHarness` rebuilds each server's forest from its executed sequence and the node's ack
  carries no chain. The harness reads the whole executed sequence, so it cannot run on a
  server that forgets, and storage figures of the compact rule count no certificate bytes. Can bias:
  certificate verdicts not at all (the forest is a function of the executed sequence); state
  transfer size and storage figures.

### Gossip and priority readings

Untagged: both nodes run only in `crates/network-node`, and no sim-tier number depends on them.

- **Alg 4, B3 reads every reply** (p. 21). "If at least one reply was received that contains x"
  is read over all r ≤ k replies, the own value is not an input, and replies carrying x₀ count
  towards the ℓ replies of the last bullet. The §3.1 equivalence with Alg 3 and the analysis of
  Theorem 4 use ℓ = 3 samples; the lemmas are lower bounds under both readings. Alg 4 therefore
  spreads x faster than Alg 3's ℓ-subset merge: with k = 6, ℓ = 3 and all six replies, a
  non-holder at holder fraction γ adopts x with probability about 1 − (1 − γ)^6, not 1 − (1 − γ)^3.
  The broadcast verification (`e3_broadcast`) runs Alg 3, so its T_B bounds the gossip node's from
  above and the two must not be pooled as one gossip time.
- **Def 3.5, subset and own value** (p. 23). The definition says "choose a subset of ℓ replies"
  and adopt the largest of them, without saying how the subset is chosen or whether the own value
  competes. The node draws a uniform ℓ-subset of the reply instances (duplicates kept) and
  excludes the own value. Other readings: the maximum over all received replies (the prose before
  Def 3.5 can be read this way, and the §6 paraphrase does), or the own value kept unless a chosen reply is
  larger (the §6 checkpoint paraphrase, which Alg 6 follows). Can bias: yes. With replies 1..6 the
  node adopts the 6 with probability 1/2, not 1, and a server holding the global maximum can drop
  to a smaller value, so priority convergence measured here is for the literal rule and is slower
  than under either alternative. Do not compare it with Alg 6's checkpoint choice (DET-16).
- **Config for every mode.** Gossip and priority take the same `Config` as Alg 1, so ℓ odd and
  k ≥ ℓ > 1 are required although Alg 4 and Def 3.5 state no such precondition (no median is
  taken). The network specs also require x ≠ x₀ and at least one holder.

### Repairs are opt-in; the default is the paper

DEV-3 (`PrefixMismatch::SkipBoundary`) and EXT-3 (`RepeatedCommit::Skip`) stay off unless a
spec turns them on. The default runs Alg 5 and Alg 6 as printed. Reasons:

- Defaults are left out of specs and ledger rows. Changing a default would silently change
  what every stored config means, and commit + seed + config would no longer regenerate a
  stored number.
- The thesis makes claims about the paper's protocol. The repairs are extensions, measured
  against that baseline on the same seeds.

A real deployment would turn both on. DEV-3 alone is not safe: a skipped boundary can let a
command commit twice, and EXT-3 is what stops it.

The default `Abort` ends a run at the first prefix mismatch of any node: the run has no safety
verdict, only an abort row. Aborts censor the runs that were about to fork, so report abort
rates next to safety rates. A T below the time the median rule needs to agree violates the
precondition of Theorem 5 (p. 27); forks and aborts there are not code deviations.

### Readings the paper's text settles (not tagged)

These follow from the paper's text. They are listed because they are easy to misread.

- L̄ membership is tested by command, not by command plus stamp, and against the growing
  merged log (a set). Otherwise a command could appear twice. KB
  `paper/compact-median-rule.md`.
- Alg 5's "performs the updates ... as described above" (fewer than ℓ replies) means: adopt S_j first, then execute the aged prefix. The
  peer's log holds only uncommitted commands. KB `paper/compact-median-rule.md`.
- b_i is read when the round's step starts, before "i sets L_i := ⊥" can change it.
- Alg 5's duplicate bullet ("if the logs in L_i contain different commands from the same
  client with the same sequence number") is part of the box (p. 26) and of §4's prose
  (p. 25). What the
  bullet leaves open is under "Equivocation ⊥" above.
- Alg 6 keeps ⊥ and the empty prefix apart: `Checkpoint::p` is `Option<Vec<_>>`. The
  initial P is ⊥. KB `paper/recovery-protocol.md`.
- The between-windows bullets run in order, not as alternatives: a reset node rolls back and then
  commits P in the same boundary. KB `paper/recovery-protocol.md`.
- Alg 6 has no duplicate-to-⊥ rule; §6 assumes unique commands (p. 30).
- P prefixes L_i by command: Alg 6 defines P as a "sequence of pre-committed commands"
  (p. 31), and the round stamps are age bookkeeping only (DET-15 fixes the rest). Executing P
  on S_i reads only commands, so P's stamps and L_i's stamps for the same commands may differ.
- If P is not a prefix of L_i at the boundary, the node panics by default (DEV-3 is the opt-in skip). The paper assumes this
  never happens w.h.p. (Lemma 6.7); the panic turns a violation into a visible abort.
- Alg 4's last bullet ("if less than ℓ values are received") overrides the one before it;
  the code runs them in box order, so ⊥ wins below ℓ replies.

## Glossary

| Paper | Code |
|---|---|
| n | number of nodes, owned by the driver |
| k, ℓ | `Config::k`, `Config::ell` (ℓ odd, k ≥ ℓ > 1, checked in `Config::new`) |
| x_i, ⊥ (value, Alg 1, 2, 4) | `x_i: Option<Value>` in `MedianNode`, `PriorityNode`, `GossipNode`; `None` is ⊥ |
| f in the (k,ℓ,f)-rule | the `f` argument of `MedianNode::step_with`; `median`, `largest` |
| x, x₀ (gossip) | `GossipNode::x`, `GossipNode::x0` |
| L_i, L_j | `l_i: Option<Log>` in `CompactNode` and `RecoveryNode` (`support/log.rs`: entries plus their sort index); `Option<Arc<Vec<Command>>>` in `LogNode`; a reply's `l_j`. `None` is ⊥ |
| an entry of L_i with its round | `Timed { entry, round }` |
| client command x | `Entry::Cmd(ClientCommand { client, sn, op })` |
| c, sn(x) | `ClientCommand::client`, `ClientCommand::sn` |
| ⊥ as a log entry (§4 duplicate rule) | `Entry::Null { client, sn }` |
| x₀ (seed command), x_d (dummy) | `Entry::Nop(0)` at round 0, `Entry::Nop(round)` (`compact::x_d`) |
| S_i, S_j | `s_i: Arc<SharedState>` (executed commands plus sn(c)); a reply's `s_j` |
| sn(c) | `SharedState::sn_get(client)`, 0 when absent |
| b_i | `CompactNode::b_i()` |
| M | the indices `m`, drawn by `draw_reply_choice` |
| L'_i | the median `logs[at]` from `merge::order_and_pick_median` |
| L'_i ∘ L̄ | `merge::median_merge` (Alg 5; returns a `log::MergedLog`); under Alg 6 `median_union` (`alg/recovery/sharing.rs`); both run `merge::merge_onto_median` |
| T | `t` in `CompactNode` (commit age) and `RecoveryNode` (window length) |
| age ≥ T, P_i | `aged_prefix_len`: the longest prefix with `round - entry.round ≥ T`; Alg 6's P_i is `checkpoint::aged_prefix` |
| R_i ∈ {no-reset, reset, ⊥} | `r_i: RState::{NoReset, Reset, Bot}` (three values, so ⊥ is `Bot`, not `None`) |
| C_i = (S, P, W) | `c_i: Arc<Checkpoint { s, p, w, certs }>`; P = ⊥ is `p: None`; a reply's `c_j` |
| C' = (S', P', W') | `replies[latest_checkpoint_index(replies)].c_j` |
| W' (next window number) | `end_window(next_window, ..)` |
| forest roots h_1..h_a (§5) | `MmrForest::roots()` |
| hc(x), p(x) | `StoredCommand::chain`, `StoredCommand::pos` |
| certificate (x, p(x), h̄c(x)) | `Certificate::Chained`; `Certificate::Newest` when x is the newest command |
| server / client §5 state | `ServerCertState`, `ClientChains` |

Not in the paper: the carried sort index (`perm`), chunked logs (`ChunkSeq`),
copy-on-write state, the draw/apply split (`draw_reply_choice` + `step_chosen`), and the
atomic counters. They exist for speed, memory and deterministic parallel stepping.
`step` equals `draw_reply_choice` followed by `step_chosen`.

## Tests

- `tests/paper/` – one file per rule, tests stated in the paper's terms.
- `tests/paper/reference.rs` – plain reference implementations of Alg 5 and Alg 6 over
  `Vec`s, with the same names and anchors as `src/alg/`; `tests/paper/differential.rs`
  compares them with the production nodes every round. Alg 1–4 need no separate
  reference: their production code already runs on plain values and `Vec`s.
- `tests/recovery_rule.rs`, `tests/paper_fidelity.rs` – Alg 6 rules and fidelity
  controls.
- `tests/support/` – the data structures in `src/support/`.

```bash
cargo test -p protocol
```
