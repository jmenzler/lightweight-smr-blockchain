# Protocol paper-fidelity controls and alternative-reading checks

This integration target combines independent passing controls and source-normative regressions with two sensitivity checks for the command-only reading of the median comparator. The paper admits (command, round stamp) as the comparison key (A5.OPEN.5), and the default uses it (DET-1). The sensitivity checks build the recipient with `StampTie::CommandsOnly` and require the command median, and they require that the default key does not produce it on the same fixture, so the fixture keeps separating the two readings. They are a sensitivity check on the alternative reading, not a fidelity requirement. They are bounded evidence, not a second protocol implementation or a universal equivalence proof.

## Module layout

`paper_fidelity.rs` is the only Cargo integration-test target. It loads `oracle.rs` and `fixtures.rs` through explicit `#[path]` modules, so the nested helper files are not standalone test targets.

## Independent command-sequence oracle

The oracle accepts exactly three labeled bare sequences. Each sequence must contain exactly one genesis `Nop(0)` at position 0, followed only by unique `ClientCommand` identities. Empty sequences, missing or duplicate genesis entries, `Null`, other `Nop` values, and duplicate commands return an error naming the offending fixture and, where applicable, the position.

After validation, the oracle compares command sequences lexicographically by the literal `(client, sn, op)` tuple and returns the middle sequence. It does not call the production median, merge, comparison, permutation-index, or tracker helpers. Round stamps are deliberately outside the oracle.

## Independently reachable donors

Both fixture families start from their own protocol constructor with the default `(6,3)` configuration and `T=100`:

- compact donors start from `CompactNode::new`, require `Triage::Amplify` for every scheduled client command, and advance through rounds 1–3 using three current self-replies plus the explicit choice `[0,1,2]`;
- recovery donors start separately from `RecoveryNode::new`, require `wants_amplify` for every scheduled command, and advance through rounds 1–3 using recovery self-replies that carry each donor's own log, checkpoint, and reset state plus the explicit choice `[0,1,2]`.

No compact output is used to initialize or prove recovery reachability. The shared literal schedule is:

| Donor | Scheduled appends | Final stamped log |
|---|---|---|
| A | `a,b` in round 2 | `seed,a@2,b@2` |
| B | `a` in round 1; `c` in round 2 | `seed,a@1,c@2` |
| C | `a,d` in round 3 | `seed,a@3,d@3` |

Each fixture exposes the three final logs and six final replies in order `A/B/C/A/B/C`. Since all transitions occur by round 3 and `T=100`, the compact states remain uncommitted; the recovery fixtures cross no boundary and likewise remain uncommitted. Recovery replies remain at the genesis checkpoint with `NoReset`.

Ignoring stamps, the command order is `A < B < C`, so the independent median is B and the required recipient prefix is exactly `seed,a,c`. Recipient assertions constrain only this prefix. They do not constrain the arbitrary union suffix or select a surviving stamp.

The compact transition shape corresponds to published-v2 Algorithm 5 (p. 26), while the independently constructed recovery reply/checkpoint/reset transitions correspond to Algorithm 6 (p. 31).

## Passing controls

The normal target has fourteen passing checks:

- three independent-oracle controls;
- two independently reachable donor controls;
- one required-prefix control across both donor families;
- one timestamp control showing that stamped logs may differ while their bare command sequences remain identical, so timestamp difference alone is not a strict command-order disagreement;
- one compact logical-isolation control holding a public immutable reply log/state handle while the sender changes its log and commits a command;
- one recovery logical-isolation control holding a public immutable reply log/checkpoint handle while the sender commits across a boundary;
- one Algorithm 3 regression requiring an unblocked bottom-log extended node to amplify a newly received command;
- one Algorithm 6 regression requiring recovery to inherit that bottom-log behavior;
- two alternative-reading sensitivity checks (below); and
- one control showing that bare `RecoveryNode::wants_amplify` is sequence-agnostic. Future-sequence rejection belongs to the simulator's ACK-enabled adaptation, not Algorithm 6 itself.

The isolation controls assert logical contents before and after mutation. They do not require a particular `Arc` identity and do not replace the shared path with test-created deep copies.

## Alternative-reading sensitivity checks

| Test | Classification | Published-v2 basis | Assertion |
|---|---|---|---|
| `command_only_median_alternative_compact` | Alternative reading (A5.OPEN.5): the comparator ignores round stamps | Algorithm 3 defines lexicographic ordering over command sequences (p. 20); Algorithm 5 applies the median to timed compact logs (p. 26), while Section 4 discusses age agreement (pp. 25, 27) without defining whether age participates in strict command ordering | With replies A/B/C selected at indices `[0,1,2]`, a `CommandsOnly` recipient starts with `seed,a,c` and a default-key recipient does not; suffix and stamp winner remain unconstrained |
| `command_only_median_alternative_recovery` | Alternative reading (A5.OPEN.5), as above | Algorithm 6 inherits the extended median rule (p. 31) and does not independently settle the timed comparator | A `CommandsOnly` recovery recipient starts with `seed,a,c` and a default-key one does not; suffix and stamp winner remain unconstrained |

## Commands

```text
cargo test -p protocol --test paper_fidelity
```

The command runs fourteen passing checks, the two sensitivity checks among them.
