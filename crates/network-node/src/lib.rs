//! Networked node for the (6,3)-median-rule protocol suite: protocol cores behind a sans-IO round engine.
//!
//! # Where the real-TCP tier departs from the paper's round model
//!
//! The paper's rounds complete every push and pull before the next round starts and lose messages
//! only to blocking (p. 4). Here a round ends on a wall-clock deadline in both sync modes (the
//! barrier only aligns round starts), so a reply or append arriving after its deadline is counted
//! (`late_rep`, `late_app`, `dropped_past`) and dropped: the node steps on fewer replies, and
//! because the ℓ-subset is drawn only from ℓ or more replies, one late reply also shifts that
//! node's later draws. Timer deadlines are absolute, so a slow `on_round_end` shortens the next
//! window and surfaces as late messages or in `t_step_compute_us`. In timer mode nodes and client driver
//! wait until the start instant before round 1, so `start_offset_ms` is a bind grace period and round 1
//! is as long as any other round. Pull requests, replies, appends and client commands are reconciled sent
//! against received (or blocked, or late) over the whole run, final round included; client acks are
//! not, since a lost ack is indistinguishable from a blocked server's silence. The collector exits 1
//! unless the network gates are clean, so only clean runs are comparable to the simulator.
//!
//! `collect` exits 0 for a clean run, 1 for an invalid one (a network gate failed) and 3 for a valid run
//! whose safety check failed; the outputs are written in every case. The safety check is a cross-server
//! executed-prefix comparison for compact runs and a held-value-was-an-initial-value check for the value
//! modes (see `collect::Gates::safety_ok`). A node whose execute-once state machine refuses an
//! out-of-sn-order commit (skip policy) is frozen, flags every later record and fails the check too; an
//! extended run has no execution layer and nothing to check.
//!
//! Round-record JSONL from an older build is incompatible by design: fields such as
//! `TrackedDigest::in_log` have no serde default, so such a file fails to parse instead of being read
//! with a guessed value.
//!
//! The tier does not run Section 5 certificates (specs with `certs` are rejected and `ClientAck`
//! carries no chain) or Alg 6 recovery, and it does not report the simulator's double-execution round:
//! the safety check sees divergence between servers, not an op executed twice identically everywhere.

pub mod cli;
pub mod client;
pub mod collect;
pub mod deploy;
pub mod digest;
pub mod engine;
pub mod harness;
pub mod net;
pub mod record;
pub mod rng;
pub mod schedule;
pub mod spec;
pub mod wire;

pub type NodeId = u32;
