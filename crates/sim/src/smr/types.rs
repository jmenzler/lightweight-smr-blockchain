//! Wire/report types shared across the SMR drivers.

use crate::{BlockSchedule, Config};
use protocol::extended::Command;
use protocol::merge::{StampTie, UnionStamp};
pub use protocol::recovery::PrefixMismatch;
pub use protocol::shared_state::RepeatedCommit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Proto {
    Extended,
    Compact {
        t_commit_rounds: u64,
    },
    Recovery {
        t_window_rounds: u64,
        /// Resend past §6 commitment until a contacted server acks; changes the stage-(b) draw pattern.
        resend_until_acked: bool,
        /// Boundary handling when P does not prefix L_i; `Abort` is the paper-faithful default.
        #[serde(skip_serializing_if = "is_abort", with = "PrefixMismatchWire")]
        prefix_mismatch: PrefixMismatch,
    },
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

fn is_zero_usize(count: &usize) -> bool {
    *count == 0
}

/// Keeps the default off the wire so existing spec and ledger bytes stay byte-identical.
pub(crate) fn is_abort(policy: &PrefixMismatch) -> bool {
    *policy == PrefixMismatch::Abort
}

pub(crate) fn is_execute(policy: &RepeatedCommit) -> bool {
    *policy == RepeatedCommit::Execute
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(remote = "RepeatedCommit", rename_all = "snake_case")]
pub(crate) enum RepeatedCommitWire {
    Execute,
    Skip,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(remote = "PrefixMismatch", rename_all = "snake_case")]
pub(crate) enum PrefixMismatchWire {
    Abort,
    SkipBoundary,
}

/// One client command: `client` resends `op` to servers from `round` on, until its stop signal:
/// the delivery signal (extended) or Alg 5's commit ack (compact, recovery without a global latch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Injection {
    pub round: usize,
    pub client: u32,
    pub op: u64,
    /// Pinned delivery target for every attempt (lab affordance); `None` = uniform draw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<u32>,
}

/// Auto-arrival id namespaces; the op base stays f64-exact because reports cross into JS.
pub const AUTO_CLIENT_BASE: u32 = protocol::compact::AUTO_CLIENT_BASE;
pub const AUTO_OP_BASE: u64 = 1 << 40;

/// The `j`-th auto arrival under the unique client model.
pub fn auto_command(j: u64) -> protocol::compact::ClientCommand {
    protocol::compact::ClientCommand {
        client: AUTO_CLIENT_BASE
            .checked_add(u32::try_from(j).expect("auto client overflow"))
            .expect("auto client overflow"),
        sn: 1,
        op: AUTO_OP_BASE + j,
    }
}

/// The traffic phase governing `round`: the last one that has started.
pub fn active_phase(phases: &[TrafficPhase], round: usize) -> Option<&TrafficPhase> {
    phases.iter().rev().find(|p| p.from_round <= round)
}

/// Which client an auto arrival comes from: a fresh one (`Unique`) or a bounded `Pool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientModel {
    #[default]
    Unique,
    /// `clients` reusable clients, at most one command in flight each.
    Pool { clients: u32 },
}

impl ClientModel {
    /// Keeps `unique` off the wire so existing spec bytes stay byte-identical.
    pub fn is_unique(&self) -> bool {
        matches!(self, ClientModel::Unique)
    }

    pub fn pool_size(&self) -> Option<u32> {
        match *self {
            ClientModel::Unique => None,
            ClientModel::Pool { clients } => Some(clients),
        }
    }
}

/// One piece of the piecewise-constant arrival process, starting at `from_round`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TrafficPhase {
    pub from_round: usize,
    pub arrivals_pmf: Vec<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SmrScenario {
    pub n: usize,
    pub seed: u64,
    pub cfg: Config,
    /// Amplification strength: `amp_count(sigma, n)` append requests (Alg 3/5 step 1).
    pub sigma: f64,
    pub proto: Proto,
    pub injections: Vec<Injection>,
    pub max_rounds: usize,
    pub schedule: BlockSchedule,
    /// Once active, every round makes exactly one count-draw; "off" is a point-mass-0 pmf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic: Option<Vec<TrafficPhase>>,
    #[serde(default, skip_serializing_if = "ClientModel::is_unique")]
    pub client_model: ClientModel,
    /// Mount the §5 certificate layer on recovery nodes; inert in every reported field.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub certs: bool,
    /// Blocks unioned onto the sampled mask after every mask draw, so the RNG stream is untouched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_blocks: Vec<ManualBlock>,
    /// One component id per node; server-to-server replies cross only within a component.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<Vec<u32>>,
    /// End the run at the round the safety latch flips.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub halt_on_violation: bool,
    #[serde(default, skip_serializing_if = "MergePolicy::is_default")]
    pub merge_policy: MergePolicy,
    /// What compact and recovery nodes do when a committed slot is committed again.
    #[serde(
        default,
        skip_serializing_if = "is_execute",
        with = "RepeatedCommitWire"
    )]
    pub repeated_commit: RepeatedCommit,
}

/// Merge tie-break knobs for compact/recovery nodes; the default is the standing resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergePolicy {
    #[serde(default, with = "StampTieWire")]
    pub stamp_tie: StampTie,
    #[serde(default, with = "UnionStampWire")]
    pub union_stamp: UnionStamp,
}

impl MergePolicy {
    /// Keeps the default off the wire so existing spec and ledger bytes stay byte-identical.
    pub fn is_default(&self) -> bool {
        *self == MergePolicy::default()
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(remote = "StampTie", rename_all = "snake_case")]
enum StampTieWire {
    IncludeRound,
    CommandsOnly,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(remote = "UnionStamp", rename_all = "snake_case")]
enum UnionStampWire {
    FirstSighting,
    LastSighting,
}

/// One targeted block over `[from_round, to_round)`; `None` runs to the horizon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualBlock {
    pub node: u32,
    pub from_round: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_round: Option<usize>,
}

impl ManualBlock {
    pub fn covers(&self, round: usize) -> bool {
        round >= self.from_round && self.to_round.is_none_or(|to| round < to)
    }
}

/// Largest σ·log₂ n a scenario may request; bounds the append fan-out of one delivery.
pub const MAX_APPEND_REQUESTS: f64 = 1_048_576.0;

// A product this close above an integer is that integer with float noise (16.6 · 15 = 249.00000000000003).
const AMP_SNAP: f64 = 1e-9;

/// ⌈σ·log₂ n⌉ (Alg 3 B1, p. 20), floored at one; the paper leaves the log base open, so base 2 is
/// this implementation's convention.
pub fn amp_count(sigma: f64, n: usize) -> usize {
    let product = sigma * (n as f64).log2();
    let nearest = product.round();
    let ceil = if (product - nearest).abs() < AMP_SNAP {
        nearest
    } else {
        product.ceil()
    };
    (ceil as usize).max(1)
}

/// Per-round observability counters for SMR runs (one CSV row each).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SmrRoundMetrics {
    pub nonbot_logs: u32,
    pub blocked: u32,
    /// Useful at the round's start (p. 19): L_i ≠ ⊥ when the round began, not blocked in it. The
    /// Def 1.4 counters read the round's end instead (`SmrReport::strong_safety_violation_rounds`).
    pub useful: u32,
    pub distinct_logs: u32,
    pub max_log_len: u32,
    pub min_executed_len: u32,
    pub max_executed_len: u32,
    /// (min, max) executed length over the servers with L_i ≠ ⊥ in this row and not blocked; None if
    /// there are none. A recovery window-end row is read before the pass.
    pub useful_executed: Option<(u32, u32)>,
    pub arrivals: u32,
    /// Common prefix of non-⊥ logs (extended); compact uses min_executed_len.
    pub lcp_len: u32,
}

/// One point of a command's broadcast curve, measured on entering-round logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SpreadPoint {
    pub round: usize,
    /// Useful servers whose log at the start of the round holds the command. Compact and recovery
    /// read L_i only, not S_i, so once the command is committed the holders drop to 0 although the
    /// useful servers executed it; the position stats below do include S_i.
    pub useful_holders: u32,
    pub useful_total: u32,
    /// Post-step position stats across the logs holding the command; for an even count the median
    /// is the upper one (index len/2 of the sorted positions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos_min: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos_med: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pos_max: Option<u32>,
}

/// A command's broadcast curve, or the fact that no one measured it; deliberately no `len`/`is_empty`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpreadCurve {
    Observed(Vec<SpreadPoint>),
    NotObserved,
}

impl SpreadCurve {
    /// The points; panics on `NotObserved`, since the caller is on the wrong runner.
    pub fn points(&self) -> &[SpreadPoint] {
        match self {
            SpreadCurve::Observed(v) => v,
            SpreadCurve::NotObserved => panic!(
                "the position census was scoped out by this runner, so there is no \
                 curve to read; use `run_smr` or `run_smr_logged` if you need it"
            ),
        }
    }

    pub fn observed(&self) -> Option<&[SpreadPoint]> {
        match self {
            SpreadCurve::Observed(v) => Some(v),
            SpreadCurve::NotObserved => None,
        }
    }
}

// Observed serializes as the bare array; NotObserved as null, distinguishable from empty.
impl serde::Serialize for SpreadCurve {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            SpreadCurve::Observed(v) => v.serialize(s),
            SpreadCurve::NotObserved => s.serialize_none(),
        }
    }
}

/// One client-stage attempt; attempt count equals the stage-(b) draw count for random targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DeliveryAttempt {
    pub round: usize,
    pub target: u32,
    pub outcome: AttemptOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum AttemptOutcome {
    /// Extended/recovery: accepted by a server. The delivery signal is a driver choice: §3 (pp. 19–20)
    /// defines no acknowledgement, only that clients resend to random servers.
    Delivered,
    TargetBlocked,
    /// Recovery: the target was reachable but held no log (⊥).
    TargetBot,
    /// Compact: Alg 5 "if L_i ≠ ⊥, sn(x) = sn(c) + 1 and x ∉ L_i".
    Amplified,
    /// Compact: Alg 5 "if L_i ≠ ⊥ and sn(x) = sn(c)".
    AckCommitted,
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum CommandStatus {
    Complete,
    Pending,
    Dead,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommandReport {
    pub client: u32,
    pub op: u64,
    pub injection_round: usize,
    /// The paper's injection (pp. 7, 19): first round a non-blocked server received the command
    /// from its client, whatever it then did with it (amplify, ack, or ignore it for L_i = ⊥, an
    /// sn gap or a held copy). Paper latency (`lat_*`) counts from here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub injected_round: Option<usize>,
    /// First receipt that started the command's spread: a non-blocked server amplified it
    /// (compact, recovery) or received it (extended, where it equals `injected_round`). The T_B
    /// origin; a recovery rollback that discards the copy does not move it.
    pub delivered_round: Option<usize>,
    /// T_B landmark: first round the command was in every non-⊥ log. A recovery rollback that
    /// later drops it from every log does not clear it.
    pub all_logs_round: Option<usize>,
    /// Lemma 3.8's T_E landmark (p. 24), T_M = prefix_fixed − all_logs: the first round at whose
    /// end every non-⊥ log (a superset of the useful servers) begins with one identical prefix
    /// through x. Stricter than Def 1.3 agreement, never earlier than `index_agreed_round`.
    pub prefix_fixed_round: Option<usize>,
    /// Def 1.3 (p. 6) read over the same logs: x at one index in every non-⊥ end-of-round log,
    /// from this round on to the end of the run, the index constant throughout. The horizon
    /// right-censors it: Def 1.3 asks for "after which", so every value is a lower bound on
    /// persistence, and one equal to the run's last round (`SmrReport::metrics.len()`) rests on
    /// a single observed round.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_agreed_round: Option<usize>,
    /// Compact: Alg 5 "if L_i ≠ ⊥ and sn(x) = sn(c)".
    pub committed_ack_round: Option<usize>,
    /// Compact: first round at whose end every useful server (as for
    /// `SmrReport::strong_safety_violation_rounds`) has executed the command (§1.2, p. 5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committed_round: Option<usize>,
    /// Recovery: first boundary round where every useful server executed the command (Lemma 6.8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executed_round: Option<usize>,
    pub auto: bool,
    pub spread: SpreadCurve,
    pub attempts: Vec<DeliveryAttempt>,
    /// Distinct servers whose append inbox received the command in its delivery round.
    pub amp_receivers: Vec<u32>,
    pub status: CommandStatus,
}

/// Which check ended the run; compact runs end only on `OutOfSnOrderCommit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum RunFailurePhase {
    /// Recovery under `Abort`: P did not prefix L_i at a window end.
    BoundaryPreflight,
    /// Lemma 6.9 (monotonicity): a server's executed sequence shrank.
    ExecutedRegression,
    /// The execute-once state machine (compact or recovery) refused a commit that skips an sn.
    OutOfSnOrderCommit,
}

/// The broken condition; untagged, so a prefix violation serializes as before.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum RecoveryViolation {
    Prefix(protocol::recovery::RecoveryPrefixViolation),
    ExecutedRegression {
        previous_len: u64,
        len: u64,
    },
    OutOfSnOrder {
        client: u32,
        sn: u64,
        committed_sn: u64,
    },
}

/// A compact or recovery run ended as a recorded failure; serialized without the type name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RunFailure {
    pub node: u32,
    pub attempted_round: usize,
    pub completed_round: usize,
    pub observed_round: usize,
    pub phase: RunFailurePhase,
    pub violation: RecoveryViolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SmrTerminal {
    /// Ran the full horizon; SMR has no absorbing exit.
    Ran { rounds: usize },
    /// Every log went ⊥; terminal, since no log-holding peer can ever reply again.
    Dead { round: usize },
    /// `halt_on_violation` stopped the run after `round`. Not the fleet `wedged_round`.
    Halted { round: usize, kind: HaltKind },
    Failed {
        attempted_round: usize,
        completed_round: usize,
        observed_round: usize,
        phase: RunFailurePhase,
    },
}

/// Why a `halt_on_violation` run stopped. An executed-state shrink and a boundary prefix
/// abort stay `Failed`, flag or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HaltKind {
    /// The no-fork latch `safety_ok` flipped: two servers' executed sequences diverged.
    Fork,
}

impl HaltKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HaltKind::Fork => "fork",
        }
    }
}

impl SmrTerminal {
    pub fn is_failed(self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmrReport {
    pub metrics: Vec<SmrRoundMetrics>,
    pub commands: Vec<CommandReport>,
    pub terminal: SmrTerminal,
    /// No fork: at every check, all n executed sequences (⊥ and blocked servers included) were
    /// prefixes of one order (compact and recovery). Not Def 1.4 Strong Safety, not Def 1.5
    /// Monotonicity. Extended runs have no commit and no checker, so it stays `true` for them and
    /// says nothing.
    pub safety_ok: bool,
    /// First round some server's executed sequence held one (client, sn) twice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec_dup_round: Option<usize>,
    /// Def 1.5 Monotonicity (p. 8), per server: (server, observation) pairs whose executed
    /// sequence was not an extension of that server's previous one. Compact observes at every
    /// round end, recovery also after every between-window pass. 0 for extended.
    #[serde(skip_serializing_if = "is_zero")]
    pub retractions: u64,
    /// The retractions that did not shorten the sequence: an executed prefix rewritten.
    #[serde(skip_serializing_if = "is_zero")]
    pub retraction_rewrites: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_retraction_round: Option<usize>,
    /// Def 1.4 Strong Safety (p. 7), read as strict equality at every round end: rounds in
    /// which two useful servers held different executed sequences. Useful, for this and every
    /// other Def 1.4 counter in compact and recovery: L_i ≠ ⊥ after the round's step (recovery:
    /// after the between-window pass when one runs) and not blocked in that round, read at the
    /// round's end. The paper leaves the instant open and reads useful at the round's start
    /// (pp. 10, 19) or as L_i ≠ ⊥ alone (p. 35). A useful server one commit behind the others
    /// counts as well, so this is a lag count, not a fork count; forks are `safety_ok`.
    #[serde(skip_serializing_if = "is_zero")]
    pub strong_safety_violation_rounds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strong_safety_first_violation: Option<usize>,
    /// Recovery: the same check at window ends only, after the pass, the instant Lemma 6.7
    /// speaks about.
    #[serde(skip_serializing_if = "is_zero")]
    pub strong_safety_boundary_violations: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strong_safety_boundary_first_violation: Option<usize>,
    /// Entries the execute-once state machine left out, summed over nodes; 0 unless it is on.
    #[serde(skip_serializing_if = "is_zero")]
    pub repeat_skips: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_repeat_skip_round: Option<usize>,
    /// DEV-3 episodes, one per node from its boundary skip to its adoption; 0 unless it is on.
    #[serde(skip_serializing_if = "is_zero")]
    pub boundary_skip_events: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_boundary_skip_round: Option<usize>,
    /// Latest episode start; an open episode that started here is censored by the horizon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_boundary_skip_round: Option<usize>,
    /// Longest completed episode in rounds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_rejoin_rounds: Option<usize>,
    /// Longest stale spell, completed or still open; a lower bound for an open one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longest_stale_rounds: Option<usize>,
    /// Nodes still stale at the end after at least one round in which they could adopt.
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub unrejoined_nodes: usize,
    /// Nodes that skipped in the final round: no round to adopt yet, so not counted as unrejoined.
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub horizon_skip_nodes: usize,
    /// First round from which no recovery node can ever hold a log again: every L_i = ⊥, every
    /// checkpoint has one W (nothing to adopt), and each node is DEV-3-stale or holds P = ⊥ (its
    /// rollback restores no log). The run still ends `Ran`; this is its liveness verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wedged_round: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_peak_in_flight: Option<u32>,
    /// Provenance, not an observation. Lean and full runs of a seed also differ in what lean sheds:
    /// the `attempts`, `amp_receivers` and `spread` of settled commands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spill_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RecoveryReport {
    /// Checkpoint-lineage fork oracle (Lemma 6.5/6.7); P is compared as commands, not stamps.
    pub fork_ok: bool,
    /// Lemma 6.7 (p. 35) on the commit path: the first window end at which some W has two
    /// distinct checkpoints (S, or P read as commands) that servers committed, at this window
    /// end or earlier ones. Checkpoints no server commits, and the lineage across windows, are
    /// `fork_ok`'s. A rollback commits the server's own P, which prefixes the L_i it restores, so
    /// the node-local prefix check cannot see this. Dated under both prefix-mismatch policies;
    /// neither stops on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_fork_round: Option<usize>,
    pub rounds: Vec<RecRoundMetrics>,
}

impl SmrReport {
    /// Smallest window end r* whose useful servers hold one executed sequence after the pass
    /// (`RecRoundMetrics::useful_agree`, Def 1.4), with reset == 0 and ≥ ⌈3n/4⌉ logs before the
    /// pass at r* and every later window end; `None` for a failed run.
    pub fn recovered_round(&self, n: usize, t_window: u64) -> Option<usize> {
        if self.terminal.is_failed() {
            return None;
        }
        let rec = self.recovery.as_ref()?;
        let floor = (3 * n).div_ceil(4) as u32;
        let boundaries: Vec<usize> = (1..=self.metrics.len())
            .filter(|&r| (r as u64).is_multiple_of(t_window))
            .collect();
        let clean =
            |b: usize| rec.rounds[b - 1].reset == 0 && self.metrics[b - 1].nonbot_logs >= floor;
        boundaries.iter().copied().find(|&r_star| {
            rec.rounds[r_star - 1].useful_agree == Some(true)
                && boundaries.iter().all(|&b| b < r_star || clean(b))
        })
    }
}

/// Per-round recovery counters; the round-mT row is pre-boundary state except `useful_agree`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RecRoundMetrics {
    pub noreset: u32,
    pub reset: u32,
    pub bot_r: u32,
    pub window: u64,
    pub rollbacks: u32,
    pub max_checkpoint_window: u64,
    pub rollback_depth: u32,
    /// Distinct (S, P) at the largest W after the boundary re-mint, P read as commands (Lemma 6.5/6.7).
    pub cp_fork_k: u32,
    pub max_cp_p_len: u32,
    /// Def 1.4 Strong Safety at this round's end, after the between-window pass in a window-end
    /// round: whether every useful server (see `SmrReport::strong_safety_violation_rounds`) holds
    /// one executed sequence; `None` when no server is useful or the round ended the run.
    pub useful_agree: Option<bool>,
}

/// Seed command x₀: lexicographically smallest, pinning position 0 of every log.
pub const SEED_COMMAND: Command = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct NodeGlance {
    /// None = ⊥.
    pub log_len: Option<u32>,
    pub executed_len: u32,
    /// Recovery only: R_i as 0 no-reset, 1 reset, 2 ⊥.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SmrStepStatus {
    pub round: usize,
    pub metrics: SmrRoundMetrics,
    pub blocked: Vec<u32>,
    pub nodes: Vec<NodeGlance>,
    /// Holders per command not yet in every log.
    pub spreading: Vec<SpreadingCommand>,
    pub dead: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rec: Option<RecStepInfo>,
}

impl SmrStepStatus {
    pub fn failed(&self) -> bool {
        self.failure.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RecStepInfo {
    pub adoptions: Vec<(u32, u32)>,
    pub reset_votes: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SpreadingCommand {
    pub client: u32,
    /// Index-free key (report order is canonical, so indices do not survive replay). The op, not
    /// the client: a pool client reused after its ack can have two commands spreading at once.
    pub op: u64,
    pub holders: Vec<u32>,
}
