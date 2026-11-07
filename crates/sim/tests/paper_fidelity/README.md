# Simulator paper-fidelity controls and expected-red contracts

This integration target contains nine normally passing controls and one ordinary ignored desired-behavior assertion. The ignored test intentionally fails on the current simulator when invoked explicitly. It is bounded evidence for the named default-policy contract, not a second simulator or a universal equivalence proof.

The target uses only public simulator interfaces and test-local literals. It has no runtime dependency on the paper reading notes (`docs/paper-spec/`), Downloads, evidence files, wall-clock time, or test order.

## Passing controls

| Test | Purpose |
|---|---|
| `control_explicit_ack_enabled_recovery_stops_after_contacted_node_ack` | An explicit `resend_until_acked:true` one-command recovery client stops only after its last contacted server returns `AckCommitted`, with the same round recorded in `committed_ack_round`. It does not require fleet-wide execution. |
| `control_sequential_client_issues_next_command_only_after_first_ack` | One client injects SN 2 only after SN 1 has a contacted-node commit acknowledgement; both commands receive their own `AckCommitted` attempt. |
| `control_unblocked_log_holder_delivers_and_amplifies_same_round` | The non-adaptation twin of the released-bottom case: a pinned, unblocked extended target enters round 2 with a log, delivers the unseen command, and records non-empty same-round amplification receivers. |
| `compatibility_legacy_recovery_client_completes_without_ack_feedback` | The explicit legacy `resend_until_acked:false` workload still completes without any `AckCommitted` feedback. This preserves the legacy mode; it does not decide the omitted-field default. |
| `control_small_recovery_full_and_lean_runs_match_logical_state` | A small ACK-enabled recovery run has identical terminal state, safety/failure state, per-round metrics, recovery counters, status, execution, and landmark fields under `run_smr` and `run_smr_lean`. Attempts, receiver vectors, and spread retention are deliberately outside the equality contract. |
| `extended_driver_released_bottom_target_amplifies_same_round` | Algorithm 3 regression: a pinned extended target that was blocked and became bottom admits and amplifies the command immediately after release (Section 3, p. 19; Algorithm 3, p. 20, versus Algorithm 5, p. 26). |
| `recovery_adaptation_future_sequence_number_has_no_amplification_receivers_through_round_32` | ACK-enabled recovery adapter regression: with SN 1 blocked, SN 2 is ignored rather than amplified or logically admitted through round 32. The bare Algorithm 6 node remains sequence-agnostic. |
| `recovery_adaptation_blocked_predecessor_has_no_premature_ack_through_round_48` | ACK-enabled recovery adapter regression: a blocked predecessor continues retrying after release and receives no premature ACK through round 48. Separate stepped engine unit tests pin precommit `P` versus committed `S` and ACK on a later non-boundary retry. |
| `theorem_budget_beta_point_one_blocks_at_most_floor_beta_n` | Definition 1.1 regression: `n=598`, `β=0.1` blocks exactly `floor(59.8)=59` servers, never the nearest integer 60 (pp. 4–5). |

The adapter keeps blocked and bottom-target filtering ahead of ACK/admission triage. Its exact committed-SN acknowledgement and next-SN admission are intentionally local to the simulator client stage; `RecoveryNode::wants_amplify` is unchanged. The adapter supports overlapping scheduled commands, but a stale earlier sequence remains unacknowledged after a successor's exact-SN acknowledgement; it does not infer an acknowledgement from `sn >= command.sn`.

## Ignored expected-red classification

| Test | Classification | Published-v2 basis | Desired assertion | Current output |
|---|---|---|---|---|
| `default_policy_omitted_ack_setting_requires_contacted_node_commit_ack` | Paper-facing default-policy/client-environment contract, separate from explicit legacy `false` workloads | Section 4 specifies retries until the contacted server acknowledges commitment (p. 25). Section 3 specifies repeated injection without an ACK rule (p. 19). Section 6 leaves the precise adaptation of Sections 4/5 to recovery unspecified (p. 30); changing this default is a project policy choice. | A recovery spec with `resend_until_acked` omitted must make at least one attempt. If activity ends before horizon 40, the exact final attempt is `AckCommitted` and its round equals `committed_ack_round`; otherwise attempts continue through round 40. Fleet `executed_round` alone cannot stop the client. | Activity ends at round 24 with `executed_round=Some(24)`, but the final attempt is `Delivered` and no contacted-node ACK signal exists. |

The default-migration expectation remains intentionally unresolved. The timing and adapter contracts above are normal controls after the exact-SN ACK/next-SN admission repair.

## Existing typed failure cutoff coverage

Typed recovery cutoff behavior remains covered by `crates/sim/tests/smr_recovery_failure.rs`, including `boundary_prefix_violation_becomes_a_typed_terminal_failure`, `failed_round_is_reported_but_not_sent_to_the_external_observer`, `failed_lean_run_retains_a_parseable_unfinished_spill`, and `failed_state_is_absorbing_and_reuses_the_cached_status`. This target does not duplicate those tests and does not describe a fleet-wide simulator failure as a paper-distributed halt.

## Commands

Normal controls:

```text
env -u SIM_SPILL -u SIM_RUNLOG cargo test -p sim --test paper_fidelity
```

The remaining expected-red contract must be run separately:

```text
env -u SIM_SPILL -u SIM_RUNLOG cargo test -p sim --test paper_fidelity default_policy_omitted_ack_setting_requires_contacted_node_commit_ack -- --exact --ignored --test-threads=1
```

The normal command currently has nine passing checks and one ignored default-policy expectation. Historical pre-fix outputs are retained under `.superpowers/sdd/paper-fidelity-conformance-tests/task-3-evidence/`.
