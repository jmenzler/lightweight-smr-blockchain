# sim

Deterministic round simulator for the protocol in `crates/protocol`. A run is fixed by its
scenario and seed: one seeded RNG stream feeds every random choice, and the stages of a round
draw from it in a fixed order. Same scenario, same seed, same bytes out.

The code splits into three parts: the simulated world, the observers that measure it, and the
tooling that runs experiments.

## World

Everything that changes a node's state or draws from the run's RNG.

- `src/consensus.rs`, `src/consensus/blocking.rs` – the single-value simulator
  (Algorithm 1) and the blocking schedules both simulators use.
- `src/smr.rs` – `SmrState`: the SMR run loop, stage (a0) traffic arrivals and stage (a)
  blocking masks, and the read-only accessors drivers and the lab call between rounds.
- `src/smr/engine/` – one round step per protocol (`engine_ext` Algorithm 3, `engine_comp`
  Algorithm 5, `engine_rec` Algorithm 6 with its between-window pass), the stages they share
  (`stages`: amplify, node applies) and the bounded client pool (`pool`).

The stage order (a0), (a)–(e) is the RNG contract; the KB doc
`.claude/knowledge/sim/smr-driver-architecture.md` lists what each stage draws.

## Observers

Read-only measurement in `src/smr/observe/`. Nothing here draws from the RNG or mutates a node:
the executed-prefix safety check (`safety`), the canonical committed order and its digest
chain (`canonical`, `canon_cursor`, `entry_index`), log inversion for landmarks (`log_index`),
the per-round metrics row (`round_metrics`), census counters (`census_count`), memory
accounting (`accounting`), the canonical spill file (`spill`) and the lab's node detail
(`detail`).

`src/smr/tracker.rs` sits between the two. A command tracker holds the client's retry state,
which the client stage reads, and the landmarks (delivered, in all logs, prefix fixed,
committed, executed) the observation pass writes.

Two observer outputs reach the world on purpose:

- In recovery, the observer decides a command's `executed_round`. That frees a pool slot, and
  without acks it stops the client resending. Both change later RNG draws.
- In lean runs, the cursor floor and the safety latch bound how far nodes forget their
  committed prefix. Forgetting drops entries a node no longer needs; it must not change a
  protocol decision.

### Inertness test

`tests/regression/observer_inertness.rs` runs the same seeded compact and recovery scenarios
with every observer on (full history, census, §5 certificates, every accessor each round),
lean with spill, and grid-lean with the census off. After every round it compares each node's
log, executed sequence, committed sequence numbers and recovery checkpoint, and at the end the
metrics rows and every command's landmarks and RNG-drawn targets. An observer that draws once
from the RNG or writes world state fails it within a round or two of the first effect.

Narrower pins: `tests/regression/smr_lean_report.rs` (lean equals full on reports),
`tests/smr_census_scope.rs` (census on equals off), `tests/smr_cert_gating.rs` (certificates
on equals off).

## Tooling

`src/tooling/` – scenario specs, grid sweeps (`sweep_grid`), the run ledger
(`.runs/sim-runs.jsonl`), analysis and probes. Runners are in `src/bin/` and `examples/`.
They run the world through `run_smr*` and `SmrState` and report what the observers measured.

## What the compact and extended runs measure

- Extended runs have no commit notion and no safety checker: `safety_ok` and the `safety` column
  read `true` for them and carry no verdict. They also have no Def 1.3 Validity oracle; the
  compact engine's logs are checked for it in `tests/paper/smr_compact.rs`, the extended rule's
  step in `crates/protocol/tests/paper/extended_rule.rs`.
- The shared state S_i is the full executed command list. Default runs never forget it, lean and
  grid runs forget a committed prefix only up to the observer's frontier, which freezes once the
  safety latch is lost. Per-server storage and state-transfer byte figures follow that
  bookkeeping, not the paper's logarithmic bound. Protocol decisions never read the list.
- The lean runner cannot re-detect a fork whose positions it has already forgotten. It agrees with
  the full runner on every forking cell in `tests/regression/smr_lean_report.rs`; no divergence is known.
- §5 certificate state lives in the driver's `CertHarness`, not in the node (see the protocol
  README). The harness needs the whole executed sequence, so certificates cannot be combined with
  forgetting, and compact storage figures count no certificate bytes.
- `index_agreed_round` (extended, Def 1.3) starts a stretch that runs to the horizon, so the
  horizon censors it; a value equal to the last round rests on one observed round.
- A spread point's `useful_holders` counts L_i only; after a command commits, compact and
  recovery curves show 0 holders. Position stats include S_i.

## Scope of the experiments

- SMR runs use oblivious blocking only: fresh-per-round, permanent, windows, per-round
  fractions, sticky and manual blocks. `SmrScenario::validate` (`src/smr/validate.rs`) rejects
  both adaptive schedules, because the SMR mask path has no hook into node state. The paper's
  guarantees hold against an adaptive 1-late adversary (Def 1.1, p. 4); an SMR result here
  covers the oblivious adversary only. Algorithm 1 runs also use adaptive schedules.
- Algorithm 1 reports the agreement round as the first round after which the value holders are
  unanimous. Def 1.2 (p. 6) tests the useful servers of a round, so its round is the reported one
  or the next (`Outcome::Agreement`). The network collector also tests holder unanimity, from the round it persists to the horizon.
- `per_round_sticky` blocks the union of a sticky set and a fresh background sample, and
  overlapping windows block the union of their nodes. The realised count (`RoundMetrics::blocked`)
  can exceed the declared fraction; report it as the realised β.
- Partitions (`partition`, the grid `splits` axis) cut links, which Def 1.1 cannot express: it
  blocks whole servers. Results under a partition measure that extension. Partitioned nodes are
  neither blocked nor non-useful in `RoundMetrics`, and an id-contiguous init aligns each
  component with its own initial values.
- A T below the time the median rule needs to agree violates the precondition of Theorem 5
  (p. 27). Forks and aborts in such runs are not code deviations. Read a result next to the
  T the run used.
- Blocking is capped per schedule only. `manual_blocks`, sticky background noise and overlapping
  windows are unioned onto the drawn mask, so the realised blocked count can exceed the nominal
  β (`metrics.blocked` records it). A manual block on an injection's pinned target models an
  adversary that sees where a client submits, which Def 1.1 rules out (p. 5).
- Every Def 1.4 counter in compact and recovery (`strong_safety_*`, compact `committed_round`,
  `recovered_round`, the per-row `useful_agree`) reads one population: a server is useful if
  L_i ≠ ⊥ after the round's step (recovery: after the between-window pass when one runs) and it
  is not blocked in that round, read at the round's end. Def 1.4 (p. 7) leaves the instant open,
  and the paper reads useful at the round's start (pp. 10, 19) or as L_i ≠ ⊥ alone (p. 35). The
  row's `useful` count, the `min_useful` column and the census keep the p. 19 round-start
  reading. Recovery's `executed_round` keeps L_i ≠ ⊥ alone, because it stops clients and so
  steers the run. `strong_safety_violation_rounds` also counts benign lag, a useful server one
  commit behind, so it is not a fork count; forks are `safety_ok`.
- Rows flag the hypothesis of Theorem 5 (p. 25): `min_useful` is the smallest per-round useful
  count and `useful_ge_n4` says whether it stayed at or above n/4. A `safety` false row with
  `useful_ge_n4` false is outside the theorem.
- `safety` and `SmrReport::safety_ok` are written by compact and recovery runs only. Extended
  (Alg 3) has no commit rule, so its rows always read true.
- σ must be at least 1 (Alg 3, p. 20), and T is at least one round. The paper's T > 2·T_B
  (p. 25) is not checked.
- Every random choice draws from one stream. A knob that changes the number of draws (T, σ,
  merge policy, partition, manual blocks) shifts all later masks and arrivals after the first
  divergence, so seed-paired runs hold the adversary fixed only until then
  (`.claude/knowledge/decisions/rng-stream-separation-dropped.md`).
- A client with several manual injections starts each from its own round. A later sequence
  number is Ignored until the earlier one commits (Alg 5's sn(c) + 1 intake, p. 26), so such a
  client pipelines instead of waiting for each commit.
- No workload produces two commands with the same (client, sn). Alg 5's ⊥ rule for such
  equivocation (p. 26) runs in the protocol unit tests only.
- The simulator counts no messages or bytes for SMR runs. Communication per command (p. 5) is
  measured on the network tier.
- Recovery landmarks `injected_round`, `delivered_round` and `all_logs_round` latch at their
  first occurrence. A rollback that later drops the command from every log does not clear
  them; the grid latency columns end at the executed or acked round, which a rollback does not
  stale.
- `committed_entries` is the longest executed sequence over all recovery nodes, a display
  construct. §6 defines committed commands per server by its own S_i (p. 35).
- Client policy is a driver choice the paper leaves open (p. 25). Extended clients make one
  attempt per round until a non-blocked server receives the command, then stop, so a command
  whose single append wave dies out stays pending (`engine_ext`). The compact client resends
  every round until acked, and the recovery client does so when `resend_until_acked` is true.

## Format changes

Specs, CSVs and ledger rows written before these changes need the commit that wrote them.

- Recovery specs: `resend_until_acked` is a required field (true is the paper's client; false
  stops on the global executed latch). Specs without it do not parse. A spec written before
  without it ran the stress client (false), so every e9 spec of Amendments 5 to 11 measured
  the stress client; to replay one, add `"resend_until_acked": false`.
- Spec parse errors name the spec kind and the failing field (for example the missing
  `resend_until_acked`) instead of "did not match any variant".
- Schedule kind `adaptive_1late` is now `adaptive_0late`. It blocks the holders at the start of
  the current round, so it was always 0-late. The old name is rejected. The 1-late adversary is
  `adaptive_alpha_late` with `alpha: 1`.
- Grid latency columns: `mean_te` and `te_*` are gone. `lat_*` measures from the paper's
  injection to the landmark named in `lat_end`. `client_lat_*` measures from the client's first
  attempt to the round its client stops resending: the commit ack (compact, and recovery with
  `resend_until_acked`), the global executed latch (recovery without it) or `prefix_fixed`
  (extended). The old `te_*` took the earliest landmark present, which for the ack recovery
  client was the executed latch, not its ack; with the window rule below, `client_lat_*`
  values differ from the old `te_*` ones.
- Command reports gain `injected_round`: the first round a non-blocked server received the
  command, also when it ignored it (L_i = ⊥, an sn gap, a held copy). `lat_*` starts there,
  no longer at `delivered_round`, which compact and recovery set only when a server amplifies
  the command; `delivered_round` keeps that meaning and stays the T_B origin (e3, rq5). Compact
  and recovery `lat_*` values change (+0.10 to +0.15 rounds in the mean in an n = 128,
  β = 0.1 comparison); extended's do not.
  A command never injected has no paper latency, so `lat_n + lat_unsettled` can be below
  `commands`.
- Latency window and censoring: only commands whose origin is at most `rounds − slowest`
  (the slowest latency settled in the run) enter `*_n`, `*_unsettled` and the quantiles. An
  in-window command that never settled now counts as +∞ in `*_p50`, `*_p90`, `*_p99` and
  `*_max`, which can read `inf`; before, it was dropped from the quantiles. `*_mean` stays
  the mean over the settled commands.
- Probe column `incons_nodes_rv` is now `off_majority_nodes_rv`; its values changed too.
- Probe column `heals_by_retraction` counts a heal when some server retracted in the rounds
  after its split appeared, through the heal round; before, only the heal round itself
  counted.
- Ledger rows: `commit` and `dirty` describe the build (protocol and sim sources). The
  working-directory values are in `cwd_commit` and `cwd_dirty`.
- Algorithm 1 `Outcome::Agreement` carries an availability: Held, Collapsing (fewer than n/3
  holders at the horizon) or Extinct. Grid rows add the label `collapsing`; only `agreement`
  (Held) counts as survived.
- Recovery reports and the ledger outcome gain `wedged_round`: the first round after which no
  node holds a log and none can adopt a newer checkpoint.
- Network-node `RoundRecord` gained `req_recv`, `app_blocked`, `late_cli` and `transport_err`,
  without defaults. Old node JSONL does not parse.
- Algorithm 1 `Split` and `WithUndecided` inits round the registered fraction exactly
  (047bad6). u0 = 0.556 at n = 10000 and 100000 now starts 5560 / 55600 valued nodes instead
  of 5561 / 55601, which re-interprets rq5-constants `alg1-floor` and e1-consensus-smoke
  `e1-cfg-floor`; the other registered grids are unchanged.
- `kl_sweep` CSV gains a `collapsing` column. A run that agreed with fewer than n/3 holders
  at the horizon is censored, no longer a death at the horizon round, and the scan stops only
  at a β where no run survived and at least one died.
- Trace and per-command JSON files are reserved like metrics files, so a name can carry a
  `-1`, `-2`, ... suffix; the ledger row holds the actual path.
- Algorithm 1 runs with a partition no longer stop at the first round in which every node
  holds one value; they run to extinction or the horizon.
- A `Scenario` built directly in Rust is validated like a parsed spec (`Scenario::validate`);
  `run` panics on an invalid one, and `SmrScenario::validate` checks the schedule.
- SMR grid CSV: `abort_kind` is no longer the last column. It is followed by the Def 1.4 / Def
  1.5 columns (`strong_safety_violation_rounds` to `first_retraction_round`), then
  `commit_fork_round`, `min_useful` and `useful_ge_n4`. `abort_kind` reads `prefix`, `mono`,
  `sn_order` (the execute-once state machine refused a commit that skips an sn) or `pool`
  (every client of a `pool` client model was in flight when an arrival needed one; the row is
  an abort instead of a crash).
- Grid key columns (β, σ, useful fraction, split) print three decimals, and a finer value
  prints in full, so two grid points never share a cell.
- Def 1.4 counters read one useful population, at the round's end (Scope, above). Compact
  `strong_safety_*` used the round-start population, recovery's ignored blocking, and
  `recovered_round` compared executed lengths before the window-end pass; it now compares
  sequences after it. Recovery rows gain `useful_agree`.
- `commit_fork_round` dates the first window end at which one W has two distinct checkpoints
  that servers committed. It used to date a server committing a checkpoint that differed from
  the first one observed for its W, even when no server ever committed that first one.
- Specs are rejected for σ < 1, for σ·log₂ n above 2^20 append requests, for a compact or
  recovery T of 0 and for a pool of more than 2^31 clients. `SmrScenario::validate` now also
  rejects a recovery T of 0 for scenarios built in Rust.
- A live `inject` is refused while its client has a later-scheduled command, since the exported
  injections would replay it under a different sequence number.
- Specs are rejected where (k, ℓ) breaks the rule `Config::check` states (ℓ odd, k ≥ ℓ > 1);
  such scenarios now fail validation instead of running.
- `mem_accounting` accepts compact cells (observer and tracker rows only; compact nodes are not
  walked), adds an `observer.exec_dup` row and prints an observer trajectory. The observer
  keeps the canonical order, the entry index and the double-execution positions only from the
  release line on (recovery) or from the lowest executed offset (compact), so they no longer
  grow with the horizon; every verdict and CSV byte is unchanged.
- A run stopped by `halt_on_violation` (default off) ends in the terminal `Halted { round,
  kind }` instead of `Ran`; kind `fork` means the no-fork latch `safety` flipped in that round.
  The grid CSV `terminal` reads `halted` (`rounds` stays the halt round) and gains two columns
  after `useful_ge_n4`, `halt_round` and `halt_kind`, empty on every other row; the ledger
  outcome's `terminal` carries the same. An executed-state shrink or a boundary prefix abort
  stays an `abort` row, flag or not. This is not the fleet `wedged_round` above. The rq2
  rounds-fixed v3 specs set the flag, so a replay reads `halted` where their rows read `ran`.

## Tests

```bash
cargo test -p sim                                    # all of it
cargo test -p sim --test regression observer_inertness
```

Integration tests are grouped into `tests/paper/` (claims stated in the paper's terms),
`tests/support/` (data structures and drivers) and `tests/regression/` (pins against
behaviour changes). Files that touch process-global state stay single-file binaries in
`tests/`.
